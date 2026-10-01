//! Google Workspace: the connector's declared contract.
//!
//! # Where every value in this file came from
//!
//! Every number, scope string, quota figure and URL here is transcribed from
//! `docs/research/integrations/google.md`, which was written from live official sources on **2026-09-27**
//! (the pages themselves carry "last updated" dates of 2026-09-03 through 2026-09-18). Nothing in this file
//! is recalled, and nothing is inferred from the shape of another vendor's API.
//!
//! **Nothing here has been exercised against Google.** No credential exists, no request has been sent, and
//! the operations below are declarations rather than verified calls. `CompatibilityVerdict::Unverified` says
//! so in the manifest itself, which is what makes that status honest rather than a placeholder.
//!
//! # What this module is, and is not
//!
//! [`GoogleConnector`] is the **declared contract**: a `ConnectorManifest`, the auth flow, and the endpoints.
//! [`client`] is the provider's **decisions without a socket** — how a response is classified, how a page is
//! followed, how a stale cursor is recognised. What is *not* here is a transport binding: this crate has no
//! HTTP stack, by design, so the code that actually sends a request is a later step that an injected
//! transport will make possible.
//!
//! # The two declarations a reader should scrutinise
//!
//! - **`classification` is `Confidential`**, which is the highest classification a *mail* connector can
//!   carry without reaching for `Restricted`. `Restricted` is reserved for health, financial, biometric and
//!   similar content, and a mailbox routinely contains all three — so a Gmail connector that declared
//!   `Restricted` would never be permitted to reach a model at all, and one that declared `Internal` would
//!   under-report. `Confidential` is the level that says "this may not leave the machine to a third-party
//!   model", which is the true and useful statement. See `classification_choice_is_the_highest_honest_one`
//!   for the argument in full.
//! - **`webhook` is `Polling { interval: Unknown }`**, and that is a **deliberate refusal to invent a
//!   number**. Google documents no minimum polling interval for Gmail; its push guide recommends falling
//!   back to `history.list` "after a period with no notifications" and states no floor. `PollingInterval`
//!   exists so that this can be said rather than fabricated — `Documented(60)` here would be a claim about
//!   Google that no source supports.
//!
//!   The reason `Push` is *not* declared is no longer that the contract cannot express Google's mechanism:
//!   [`SignatureAlgorithm::OidcIdToken`](crate::webhook::SignatureAlgorithm::OidcIdToken) and
//!   [`EchoedChannelToken`](crate::webhook::SignatureAlgorithm::EchoedChannelToken) now name Gmail's Pub/Sub
//!   bearer JWT and Calendar's echoed channel token. The reason is that **one connector holds one
//!   `WebhookSupport` value, and Google's two mechanisms are two** — an OIDC JWT in `Authorization` with an
//!   account-in-body binding, and a channel token in `X-Goog-Channel-Token` with an account-in-header binding.
//!   Declaring one would be as incomplete as declaring neither, and `polling` is what is true of the connector
//!   as a whole. Declaring `Push` would also demand webhook signature/replay readiness items the connector
//!   cannot yet satisfy, because **neither verifier is built** — the authenticator can now be *named*, which is
//!   a contract fact, but nothing compares it (`P5-010` owns that).

pub mod channel;
pub mod client;
pub mod connection;
pub mod credential;
pub mod definitions;
pub mod http;
pub mod operations;
pub mod pubsub;
pub mod recovery;
pub mod request;
pub mod revocation;
pub mod routing;
pub mod scopes;
pub mod teardown;
pub mod token;
pub mod transport;
pub mod watch;

use crate::auth::{AuthError, AuthFlow, AuthMethod, PkceMethod};
use crate::authorization::{LoopbackHost, LoopbackRedirect, RedirectError};
use crate::manifest::{
    AuthMethodDeclaration, Classification, CompatibilityStatus, CompatibilityVerdict,
    ConnectorError, ConnectorId, ConnectorManifest, ConnectorOperation, ConnectorVersion,
    DataResidency, DocumentationLink, DocumentationLinks, LinkKind, MIN_SUPPORTED_JARVIS_VERSION,
    PollingInterval, ProviderIdempotency, ResearchRecord, ResidencyVerification, SecretField,
    WebhookSupport,
};
use crate::ratelimit::{QuotaCost, RateLimit, RateLimitEvidence, RateLimitScope, RateLimitUnit};
use jarvis_tools::ToolEffect;

/// The connector identifier, which is also the first segment of every tool namespace it registers.
///
/// `repository-layout.md`'s integration shape names the module `google/`, and a tool identifier derives from
/// the connector's namespace, so `google.gmail_messages_read` is the canonical id a policy decision names.
/// The operation segments use underscores rather than dots because an operation id becomes a **tool name
/// segment**, and `jarvis-tools` forbids a dot in the name half (`from_parts` splits on the last dot, so
/// `google.gmail.messages.read` would re-parse with the namespace `google.gmail.messages`).
pub const CONNECTOR_ID: &str = "google";

/// The repository-relative path of the record this contract was transcribed from.
pub const RESEARCH_RECORD: &str = "docs/research/integrations/google.md";

/// The date the research record was last verified against live sources.
///
/// The same date as the record's own `last_verified`, and the two are asserted equal by
/// `the_manifest_points_at_a_record_that_exists_at_the_declared_date` — a manifest whose pointer and record
/// disagree about the date would let a stale contract read as a fresh one.
pub const RESEARCH_VERIFIED_ON: &str = "2026-09-27";

/// Gmail's **restricted** read-and-organise scope.
///
/// Restricted rather than sensitive, which is the fact that governs what deploying this connector costs.
/// The documentation's own advice is to request the least permissive scope that works, and
/// `gmail.readonly` is that scope for a read-only connector — `gmail.modify` additionally permits changing
/// labels, and `mail.google.com/` permits permanent deletion bypassing the trash.
pub const SCOPE_GMAIL_READONLY: &str = "https://www.googleapis.com/auth/gmail.readonly";

/// Calendar's read scope.
///
/// Recorded without a sensitivity category, because Google's Calendar page lists its scopes **without**
/// stating one — see unresolved question 2 in the research record. Nothing here claims a category.
pub const SCOPE_CALENDAR_READONLY: &str = "https://www.googleapis.com/auth/calendar.readonly";

/// The `OpenID` Connect identity scope, used to verify the account from the provider.
///
/// `tools-and-connectors.md` requires "account identity verified from the provider, not user-entered
/// labels". [`SCOPE_GMAIL_READONLY`] and [`GoogleConnector::operations`] (the `gmail_profile_read` operation)
/// are what satisfy that requirement: the `users.getProfile` response carries an `emailAddress`, and that
/// operation is declared above and reads it.
///
/// # `openid` is **not** what makes the profile readable, and this doc said it was
///
/// An earlier version of this comment read "*the `users.getProfile` response carries an `emailAddress`, and
/// that is the operation this scope exists for*". That is wrong, and the `users.getProfile` reference is
/// explicit about it: the operation requires *"one of the following OAuth scopes"* — `mail.google.com/`,
/// `gmail.modify`, `gmail.compose`, `gmail.readonly`, `gmail.metadata` — and **`openid` is not among them**.
/// So a caller holding only `openid` would be refused by `getProfile`, and the identity the doc promised would
/// not arrive. The Gmail read scope is what the profile read needs; `openid` is requested for a different
/// reason, recorded below (`ADR-0096`).
///
/// # So what is this scope for, and why is it still requested
///
/// It is an **OIDC** scope rather than a Gmail one, and requesting it changes the token response: Google
/// returns an `id_token`, which [`crate::token::TokenResponse::has_id_token`] records arriving and
/// [`crate::google::token`] deliberately does **not** verify. Two facts about it, both of which are limits
/// rather than capabilities:
///
/// - The `id_token` is **received and unverified**. What would verify it is `jwks_uri` from the discovery
///   document, and no code does that.
/// - `openid` is requested together with the `nonce` [`AuthorizationTransaction`] already generates, which is
///   what makes a *future* ID-token check possible — the `nonce` is carried to the grant for exactly that
///   reason and nothing compares it today. So the pairing is a **prepared seam, not a working feature**, and
///   this comment says which so a reader does not mistake one for the other.
///
/// Requesting a scope whose only consumer is unbuilt is a real cost — it is one more thing on the consent
/// screen — and it is kept because dropping it would remove the `nonce`'s purpose and make a later ID-token
/// check impossible without a fresh consent for every existing account.
///
/// [`AuthorizationTransaction`]: crate::authorization::AuthorizationTransaction
pub const SCOPE_OPENID: &str = "openid";

/// Gmail's published scope categories, transcribed from the scopes page read on
/// [`SCOPE_CATEGORIES_RECORDED_ON`](scopes::SCOPE_CATEGORIES_RECORDED_ON).
///
/// # Why a table rather than a category per constant
///
/// The page publishes three lists — non-sensitive, sensitive, restricted — and the **list a scope is in** is
/// the fact, so a table keyed by scope string is the transcription and a category stored beside each constant
/// would be a second copy of it. It also makes the accounting check meaningful: a scope the manifest declares
/// but this table does not list is visible as a gap rather than silently defaulting to a category.
///
/// # The three entries that carry the surprises
///
/// - **`gmail.labels` is the only generally useful non-sensitive Gmail scope.**
/// - **`gmail.send` is sensitive, not restricted.** A send is the one operation `P5-009` must declare as
///   `ProviderIdempotency::Unknown`, and its category is a *review* fact rather than a write-privilege one.
/// - **`gmail.metadata` is restricted**, which matters because it is the least privileged way to read a
///   mailbox and an author would reasonably assume it is the cheap option. It is not.
///
/// Only the six Gmail scopes this project has a use for are listed. A partial table is deliberate: it is
/// honest about what has been read, and every omitted scope becomes
/// [`ScopeCategory::Unknown`](scopes::ScopeCategory::Unknown) in the accounting rather than being
/// guessed at.
#[must_use]
pub fn gmail_scope_categories() -> Vec<(&'static str, scopes::ScopeCategory)> {
    use scopes::ScopeCategory;
    vec![
        // ---- Non-sensitive ----
        (
            "https://www.googleapis.com/auth/gmail.labels",
            ScopeCategory::NonSensitive,
        ),
        // ---- Sensitive ----
        (
            "https://www.googleapis.com/auth/gmail.send",
            ScopeCategory::Sensitive,
        ),
        // ---- Restricted ----
        // The scope this connector actually requests for mail.
        (SCOPE_GMAIL_READONLY, ScopeCategory::Restricted),
        (
            "https://www.googleapis.com/auth/gmail.metadata",
            ScopeCategory::Restricted,
        ),
        (
            "https://www.googleapis.com/auth/gmail.modify",
            ScopeCategory::Restricted,
        ),
        ("https://mail.google.com/", ScopeCategory::Restricted),
    ]
}

impl GoogleConnector {
    /// Builds the Google connector manifest.
    ///
    /// # Errors
    ///
    /// Returns [`ConnectorError`] if any declaration below is rejected by the manifest's cross-field rules.
    /// The function exists so that a defect in this file is a **test failure at the declaration** rather
    /// than something discovered when an operator tries to install it.
    pub fn manifest() -> Result<ConnectorManifest, ConnectorError> {
        ConnectorManifest::new(
            ConnectorId::new(CONNECTOR_ID)?,
            ConnectorVersion::new("1.0.0")?,
            "Google Workspace",
            "Google LLC",
            Self::operations(),
            Self::auth_methods(),
            // See the module doc: polling with an interval nobody has established. `Push` is **not** used, but
            // the reason is no longer that the contract cannot express Google's mechanism — the authenticator
            // variants now name both of them. The reason is cardinality: this manifest has **one** `webhook`
            // field, Google's push is **two** mechanisms (an OIDC bearer JWT with an account-in-body binding,
            // and an echoed channel token with an account-in-header binding), and neither verifier is built, so
            // declaring `Push` would be as incomplete as declaring neither and would also demand webhook
            // signature/replay readiness items nothing can satisfy. `polling` is true of the connector today.
            WebhookSupport::Polling {
                interval: PollingInterval::Unknown,
            },
            Self::secret_fields(),
            Classification::Confidential,
            residency(),
            links()?,
            research()?,
            compatibility(),
        )
    }

    /// Returns the operations this connector declares.
    ///
    /// Five read operations, each mapped from a documented Gmail or Calendar method. The `id` is a
    /// **JARVIS name segment** and not the provider's method name, because policy binds to the canonical
    /// identifier; the provider's method is named in the description so a reader can find it.
    ///
    /// Every one is `ReadOnly` at risk 0, which is `tools-and-connectors.md`'s own guidance for "read
    /// calendar, search mail metadata" and makes them all `auto when scoped`. The fifth is
    /// `gmail_profile_read`, and it is the one that makes the account-identity requirement satisfiable: the
    /// four mail and calendar reads return *content*, and none of them says **which mailbox** answered, so
    /// without it a `VerifiedAccount` had no provider-supplied identifier to carry. No write operation is
    /// declared: `P5-009` owns them "behind policy and approval", and `P5-004` recorded that a Gmail send
    /// returning HTTP 200 does **not** mean the mail was sent — a fact that needs its own declaration
    /// (`ProviderIdempotency::Unknown` at minimum) rather than being added here as an afterthought.
    #[must_use]
    pub fn operations() -> Vec<ConnectorOperation> {
        vec![
            ConnectorOperation {
                id: "gmail_messages_read".to_owned(),
                description: "Read Gmail messages by id, including headers and body. Wraps \
                              `users.messages.get`; costs 20 quota units per call, making it the most \
                              expensive read in this connector."
                    .to_owned(),
                effects: vec![ToolEffect::ReadOnly],
                risk: 0,
                required_scopes: vec!["mail.read".to_owned()],
                idempotency: ProviderIdempotency::Declared,
                // 20 units: the most expensive read this connector declares, and the figure the research
                // record uses to derive the `5 + 20N` full-sync estimate.
                quota_cost: QuotaCost::Documented(20),
                rate_limit: Some(gmail_rate_limit()),
            },
            ConnectorOperation {
                id: "gmail_messages_list".to_owned(),
                description: "List Gmail message identifiers matching a query. Wraps `users.messages.list`; \
                              costs 5 quota units per call and pages with `nextPageToken`. A full sync costs \
                              roughly `5 + 20N` units for N messages, which is why the two operations are \
                              separate."
                    .to_owned(),
                effects: vec![ToolEffect::ReadOnly],
                risk: 0,
                required_scopes: vec!["mail.read".to_owned()],
                idempotency: ProviderIdempotency::Declared,
                quota_cost: QuotaCost::Documented(5),
                rate_limit: Some(gmail_rate_limit()),
            },
            ConnectorOperation {
                id: "gmail_history_list".to_owned(),
                description: "List the changes to a mailbox since a history position. Wraps \
                              `users.history.list`; costs 2 quota units per call and returns the mailbox's \
                              new `historyId`. A `startHistoryId` outside the retained range answers HTTP \
                              404, which requires a full sync rather than a retry."
                    .to_owned(),
                effects: vec![ToolEffect::ReadOnly],
                risk: 0,
                required_scopes: vec!["mail.read".to_owned()],
                idempotency: ProviderIdempotency::Declared,
                // 2 units, not 20: the per-call cost is 10× a message read, which is exactly the kind of
                // difference a single shared "read limit" would erase. The **limit** is shared because it is
                // one provider ceiling; the **cost** is per operation because it is a property of the method.
                quota_cost: QuotaCost::Documented(2),
                rate_limit: Some(gmail_rate_limit()),
            },
            ConnectorOperation {
                id: "calendar_events_read".to_owned(),
                description: "Read events from a calendar. Wraps `events.list`; supports incremental sync \
                              through the provider's `syncToken`, whose invalidation arrives as HTTP 410."
                    .to_owned(),
                effects: vec![ToolEffect::ReadOnly],
                risk: 0,
                required_scopes: vec!["calendar.read".to_owned()],
                idempotency: ProviderIdempotency::Declared,
                // **Unstated, and that is the honest value.** Google's Gmail quota page publishes no
                // Calendar cost ("not published on this page" in the research record), and defaulting to 1
                // would invent a figure the provider never stated — the over-planning failure `QuotaCost`
                // exists to prevent.
                quota_cost: QuotaCost::Unstated,
                rate_limit: None,
            },
            ConnectorOperation {
                id: "gmail_profile_read".to_owned(),
                description: "Read the authenticated mailbox's own address and current history position. \
                              Wraps `users.getProfile`; this is the operation that supplies the account \
                              identity `tools-and-connectors.md` requires to come from the provider rather \
                              than from a user-entered label."
                    .to_owned(),
                effects: vec![ToolEffect::ReadOnly],
                risk: 0,
                required_scopes: vec!["mail.read".to_owned()],
                idempotency: ProviderIdempotency::Declared,
                // 1 unit: the cheapest call this connector makes, and the figure the research record's cost
                // table already carries for `getProfile`. Recorded here with the same evidence discipline as
                // the other three — the number is the provider's.
                quota_cost: QuotaCost::Documented(1),
                rate_limit: Some(gmail_rate_limit()),
            },
        ]
    }

    /// Returns the auth methods this connector supports.
    ///
    /// One method, `oauth_pkce`, because that is what `tools-and-connectors.md` requires for a user-facing
    /// public client and what the `P5-002` flow implements. A second method is *representable* and is
    /// deliberately not declared: `ServiceAccount` with domain-wide delegation is the shape a multi-mailbox
    /// deployment would need, and the research record's unresolved question 5 records that a service account
    /// is **counted as one user** for quota — so declaring it before that is understood would be declaring a
    /// method whose throughput ceiling nobody has measured.
    ///
    /// The scopes are in the **provider's own spelling**, which [`AuthMethodDeclaration::scopes`] documents
    /// as the deliberate exception: these strings go into an authorization request, so a lossy round trip
    /// through JARVIS vocabulary would be a wrong grant. The `Scope` values in `required_scopes` above are
    /// the *different* thing — what a caller must hold.
    #[must_use]
    pub fn auth_methods() -> Vec<AuthMethodDeclaration> {
        vec![AuthMethodDeclaration {
            method: AuthMethod::OAuthPkce,
            purpose: "Sign in with a Google account and grant read access to mail and calendar. The flow \
                      returns to a loopback listener and is completed with PKCE, so no client secret is \
                      stored."
                .to_owned(),
            scopes: vec![
                SCOPE_OPENID.to_owned(),
                SCOPE_GMAIL_READONLY.to_owned(),
                SCOPE_CALENDAR_READONLY.to_owned(),
            ],
            required: true,
        }]
    }

    /// Returns the secret fields a deployment must supply.
    ///
    /// **Empty, and that is a decision.** A PKCE public client has no client secret to store: `P5-002`'s
    /// `AuthMethod::requires_client_secret()` is false for `OAuthPkce`, and Google's own documentation for
    /// native applications has the client identify itself by a client *identifier*, which is public. The
    /// refresh material does not appear here either, because a `SecretField` is what an **operator pastes**
    /// and a refresh token is what the authorization flow **receives** — it goes to the secret store through
    /// `P5-002`'s `TokenSet`, which records a `SecretRef` and never the token value.
    ///
    /// A future `ServiceAccount` method would add a `PrivateKey` field here, which is the case this list
    /// exists to be able to express.
    #[must_use]
    pub fn secret_fields() -> Vec<SecretField> {
        Vec::new()
    }

    /// The authorization endpoint, from Google's discovery document.
    ///
    /// Transcribed from `https://accounts.google.com/.well-known/openid-configuration`, which is
    /// **machine-readable** — so this is the server's own published configuration rather than a page's
    /// example. `the_endpoints_match_the_discovery_document` asserts both endpoints in this module against
    /// those recorded values, so a drift is a failing test rather than a silent wrong URL.
    #[must_use]
    pub const fn authorization_endpoint() -> &'static str {
        "https://accounts.google.com/o/oauth2/v2/auth"
    }

    /// The token endpoint, from the same document.
    ///
    /// **Not the authorization endpoint's host**: `accounts.google.com` serves the consent screen and
    /// `oauth2.googleapis.com` serves the exchange. Two different hosts for one flow is unusual enough that
    /// a reader might "correct" it, so the discovery document is named as the source.
    #[must_use]
    pub const fn token_endpoint() -> &'static str {
        "https://oauth2.googleapis.com/token"
    }

    /// The revocation endpoint, from the same document.
    ///
    /// Recorded because `P5-002`'s `RevocationKind` needs somewhere to revoke against, and because the
    /// provider's blast radius is unusual: revocation removes the project's grants, not only one account's.
    #[must_use]
    pub const fn revocation_endpoint() -> &'static str {
        "https://oauth2.googleapis.com/revoke"
    }

    /// The redirect URI this connector **registers** with Google.
    ///
    /// The **portless** loopback form, which is what `P5-002`'s `LoopbackRedirect::registered` produces and
    /// what `LoopbackRedirect::matches_except_port` compares a listener against. RFC 8252 §7.3 requires the
    /// server to accept any port at request time, so the registered form carries none.
    ///
    /// **One fact here is not established.** Google's native-app page shows the *exchange request* using a
    /// ported URI (`redirect_uri=http://127.0.0.1:9004`) and states that `redirect_uri` must match an
    /// authorized URI exactly, but it does not state which string the console accepts as the **registered**
    /// value for a Desktop-app client. See Unresolved Question 7 in the research record. This constant is
    /// therefore the form the flow's own rules produce rather than a value confirmed against the console, and
    /// `the_registered_redirect_is_the_portless_form_the_flow_requires` records that distinction.
    ///
    /// # Errors
    ///
    /// Returns [`RedirectError`] if the constant is not a valid loopback redirect — which would be a defect in
    /// this file rather than at runtime.
    pub fn registered_redirect() -> Result<LoopbackRedirect, RedirectError> {
        LoopbackRedirect::registered(LoopbackHost::V4, "/")
    }

    /// The connector's auth flow, constructed from the verified endpoints.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError`] if the flow is inconsistent. `AuthFlow::new` requires an `https://` authorization
    /// endpoint, PKCE for an OAuth method, and a redirect URI; all three are supplied from the discovery
    /// document and the loopback rules rather than chosen here.
    pub fn auth_flow() -> Result<AuthFlow, AuthError> {
        // The redirect comes from `registered_redirect`, whose failure is a defect in this file rather than
        // something a caller can act on — so it is reported as a flow inconsistency instead of being
        // propagated as a `RedirectError`, which `AuthError` has no conversion from and should not gain one
        // for (a redirect error is about a URI, and a flow error is about the flow's *shape*).
        let redirect = Self::registered_redirect().map_err(|_| AuthError::Flow {
            reason: "the registered loopback redirect is not a valid `http` URI on a loopback literal, which \
                     is a defect in this connector's own constant rather than a caller's input",
        })?;
        AuthFlow::new(
            AuthMethod::OAuthPkce,
            Some(PkceMethod::S256),
            Some(redirect.as_uri()),
            Self::authorization_endpoint(),
        )
    }
}

/// The rate limit Gmail's read path runs under.
///
/// `Documented` because the figures are on a page the manifest links, and **`PerClient`** because Google
/// quota is per *Cloud project* — 1,200,000 units/minute — which is shared by every account the connector
/// reads through. That choice matters beyond accuracy: `RateLimitScope::is_shared_between_accounts()` is
/// true for `PerClient`, so a scheduler will not report one account's exhaustion as that account's own
/// problem. A future per-account limit would be a second entry rather than a correction to this one.
///
/// # One limit for every Gmail read, and `CostUnits` is why that is correct
///
/// `per_window` is the **per-minute project allowance** and `burst` is the tighter **per-user** ceiling.
/// Both are published in **quota units**, which is what [`RateLimitUnit::CostUnits`] records — so this is
/// deliberately **one** limit shared by every Gmail operation rather than one per method. A previous version
/// had two functions, `gmail_read_rate_limit` and `gmail_history_rate_limit`, that were byte-identical while
/// the second's doc claimed its existence rested on "different documented costs". The costs do differ (20
/// against 2), but a per-call cost is not a property of a *rate limit* at all: it belongs to the operation,
/// which is where [`QuotaCost`] now carries it. Two limits for one provider ceiling was a duplicate wearing
/// the name of a distinction, and the distinction it named was real but lived in the wrong place.
///
/// The daily threshold of 80,000,000 units — which **cannot be raised** — is recorded in the research record
/// rather than here, because `RateLimit` has no daily window.
fn gmail_rate_limit() -> RateLimit {
    RateLimit {
        per_window: 1_200_000,
        window_seconds: 60,
        burst: 6_000,
        unit: RateLimitUnit::CostUnits,
        scope: RateLimitScope::PerClient,
        evidence: RateLimitEvidence::Documented,
    }
}

/// Where Google processes and stores the data.
///
/// `Unverified`, not `Verified`, and that is the honest value: the research record lists no residency page
/// among the sources it read, so nothing checked this against Google's terms. `NotStated` would be a claim
/// that Google does not state a position, which is a different and unsupported assertion.
fn residency() -> DataResidency {
    DataResidency {
        note: "Mail and calendar content is processed by Google's Workspace APIs. A deployment handling \
               regulated data must verify the relevant Workspace data-region and processing terms itself; \
               this manifest does not claim a region."
            .to_owned(),
        verification: ResidencyVerification::Unverified,
    }
}

/// The compatibility claim.
///
/// `Unverified`, because nothing here has been run against Google. `CompatibilityVerdict::is_installable()`
/// is false for `Unverified`, which is the fail-closed direction `security.md` requires — and it means this
/// manifest is a **contract under construction** rather than something an operator can install today. That
/// is the truthful state of `P5-005`'s first half, and it is better stated here than discovered by someone
/// who installs it.
fn compatibility() -> CompatibilityStatus {
    CompatibilityStatus {
        minimum_jarvis_version: MIN_SUPPORTED_JARVIS_VERSION.to_owned(),
        status: CompatibilityVerdict::Unverified,
        note: Some(
            "No request has been sent to Google and no credential exists. The declarations are transcribed \
             from dated official sources; the operations are not implemented and the contract fixtures are \
             not recorded."
                .to_owned(),
        ),
    }
}

/// The research pointer.
///
/// # Errors
///
/// Returns [`ConnectorError`] if the path or date is malformed.
fn research() -> Result<ResearchRecord, ConnectorError> {
    ResearchRecord::new(RESEARCH_RECORD, RESEARCH_VERIFIED_ON)
}

/// The documentation links.
///
/// **Only one link per `LinkKind` is permitted** (`DocumentationLinks::new` refuses a duplicate), and that
/// is a real constraint for this connector: Google publishes a separate documentation set, quota page, and
/// identity guide *per API*, and Gmail and Calendar disagree about where each lives. The chosen entry is the
/// one Google's own product navigation calls the contract, and the per-API pages are enumerated in the
/// research record instead. Recorded rather than worked around, because the alternative — picking whichever
/// looked more authoritative — would lose the other API's page silently.
///
/// `llms_txt` is deliberately **absent**, because Google has none: both candidate URLs returned HTTP 404 and
/// the research record says so. The research obligation is discharged by the documentation links.
///
/// # Errors
///
/// Returns [`ConnectorError`] if the links are rejected — including if none discharges the research
/// requirement, which is why at least one `Documentation` kind is present.
fn links() -> Result<DocumentationLinks, ConnectorError> {
    DocumentationLinks::new(vec![
        DocumentationLink {
            kind: LinkKind::Documentation,
            url: "https://developers.google.com/workspace/gmail/api/guides".to_owned(),
            purpose:
                "The Gmail API contract: resource model, terms, and the push-notification and \
                      incremental-sync guides."
                    .to_owned(),
        },
        DocumentationLink {
            kind: LinkKind::RateLimits,
            url: "https://developers.google.com/workspace/gmail/api/reference/quota".to_owned(),
            purpose:
                "Quota units per project and per user, the per-method cost table, and the daily \
                      billing threshold that cannot be raised."
                    .to_owned(),
        },
        DocumentationLink {
            kind: LinkKind::Authentication,
            url: "https://developers.google.com/workspace/gmail/api/auth/scopes".to_owned(),
            purpose:
                "Gmail's scope list, and which scopes are non-sensitive, sensitive, or restricted."
                    .to_owned(),
        },
        DocumentationLink {
            kind: LinkKind::Webhooks,
            url: "https://developers.google.com/workspace/gmail/api/guides/push".to_owned(),
            purpose:
                "Gmail push notifications through Cloud Pub/Sub: `users.watch`, the seven-day \
                      renewal bound, and the one-event-per-second cap."
                    .to_owned(),
        },
        DocumentationLink {
            kind: LinkKind::Terms,
            url: "https://developers.google.com/terms/api-services-user-data-policy".to_owned(),
            purpose:
                "The user-data policy a deployment is bound by, including the extra requirements \
                      restricted scopes carry."
                    .to_owned(),
        },
    ])
}

/// The Google connector's declarations, as a namespace type.
///
/// A unit struct rather than a module of free functions, because `GoogleConnector::manifest()` reads as the
/// name an operator and a test both use while a bare `manifest()` in this module would be ambiguous with
/// every other connector's. There is no state: each member is an associated function returning a fresh value,
/// so two callers cannot disagree about a shared one.
#[derive(Clone, Copy, Debug)]
pub struct GoogleConnector;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
