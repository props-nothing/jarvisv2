//! Connector contracts: what a provider integration must state before any provider code exists.
//!
//! `P5-001` requires "connector manifest, account, auth flow, health, sync cursor, webhook, rate-limit,
//! scope, and diagnostics contracts", and `docs/architecture/tools-and-connectors.md` names what a connector
//! owns: "manifest metadata and official documentation links; account setup and connectivity verification;
//! OAuth/API-key/service-account flow; token refresh, revocation, reauthorization, and scope changes;
//! provider client, pagination, rate limits, retry classification, and request IDs; webhook/subscription
//! lifecycle and incremental sync cursors; operation-to-tool registration; health and secret-redacted
//! diagnostics; sanitized contract fixtures and opt-in live tests."
//!
//! This crate is the **contract half** of that list and nothing else. It defines types, invariants, and
//! refusals; it performs no HTTP, holds no credential, opens no browser, and registers no tool. That split
//! is deliberate and follows `jarvis-mcp`'s precedent: the authority rules are verified as functions of
//! their arguments, with no provider SDK and no adapter in scope.
//!
//! # The one rule that decides most of the design
//!
//! > A connector manifest is parseable **without loading provider code**
//! > (`docs/architecture/tools-and-connectors.md`), so onboarding, `doctor`, docs, and tool discovery can
//! > run without initializing every SDK.
//!
//! That is not a performance note; it is a **trust** note. It means the manifest is the only thing an
//! operator reads before installing a connector, so it is also the only thing that can warn them, and it
//! must be able to do so completely on its own. Three consequences run through this crate:
//!
//! - [`ConnectorManifest`] carries the **effect metadata, scopes, and classifications** rather than
//!   deferring them to code, because an operator deciding whether to grant Gmail access needs to see what
//!   the connector will do before anything is installed.
//! - The manifest holds **research links and a compatibility status**, so a connector whose vendor changed
//!   its API has a place to say so — `docs/development/external-research.md` makes a dated record mandatory,
//!   and a manifest with no `llms.txt`/docs links is a connector nobody can re-verify.
//! - Nothing in the manifest is a **secret**. It names the secret *fields* a deployment must supply
//!   ([`SecretField`]) and never a value, which is the mechanical form of `security.md`'s "no token material
//!   in model context, URLs/logs, diagnostics, or normal database columns".
//!
//! # Why every field is required rather than optional
//!
//! Every type here is a struct of required fields with no `Default` and no `Option`, except where absence is
//! itself a documented state. `AGENTS.md`'s recurring defect class is "two values that must agree, with
//! nothing holding both"; the cheaper version of that class is a field a caller can omit, because **an
//! omitted field reads as a satisfied one**. `P3-001`'s `EffectSet` is not `derive(Deserialize)` for exactly
//! this reason, and `jarvis-models::Normalization` defaults to `Unknown` rather than to the assumption a
//! deserializer would prefer. So: the effect set is required, the auth methods are required, the scopes are
//! required, and where absence is legitimate it is a **named variant** ([`WebhookSupport::Unsupported`],
//! [`ConnectorHealth::Unknown`]) rather than a missing field.
//!
//! # Serialization, and what is deliberately not deserializable
//!
//! [`ConnectorManifest`] is `Deserialize` because it is authored as a document — a manifest is data, and a
//! manifest-driven connector is the point of `P5-003`. `EffectSet` is not, because a derived impl makes an
//! empty effect set representable and that is the one value the type exists to exclude (`P3-001`), so the
//! manifest carries the raw [`ToolEffect`] list and converts through `EffectSet::new`, which refuses an
//! empty set. A malformed manifest is therefore a **parse error rather than a silently empty capability**,
//! which is the same reasoning `jarvis-tools`' `RetryDeclaration` records.

mod account;
mod auth;
mod authorization;
mod cursor;
mod diagnostics;
mod health;
mod manifest;
mod ratelimit;
mod readiness;
mod scaffold;
mod token;
mod webhook;

/// A provider connector's declared contract.
/// Exported as a **namespace per provider** rather than re-exported into the crate root, and that is forced
/// by the second connector: `jarvis_connectors::CONNECTOR_ID` would be ambiguous the moment Microsoft
/// arrives, and a crate root holding one provider's scope constants would make the others' hard to find.
/// `repository-layout.md`'s integration shape is one module per connector, so the path mirrors it —
/// `jarvis_connectors::google::GoogleConnector::manifest()`.
pub mod google;

/// `application/x-www-form-urlencoded`, in both directions.
///
/// Exported rather than private because the encoder has **no caller yet** — it is what the token endpoint's
/// form `POST` body needs — and a `pub` item inside a private module is unreachable and therefore dead code.
/// `authorization` uses the decoder today; the encoder is the next slice's.
pub mod form;

pub use account::{
    AccountReference, AccountStatus, MAX_ACCOUNT_DISPLAY_NAME_CHARS, MAX_PROVIDER_ACCOUNT_ID_CHARS,
    VerifiedAccount,
};
pub use auth::{
    AuthCallback, AuthChallenge, AuthError, AuthFlow, AuthMethod, AuthState, ChallengeAction,
    PkceChallenge, PkceMethod, PkceVerifier, RefreshOutcome, ScopeChange, ScopeSetChange,
    SecretValue,
};
pub use authorization::{
    AuthRefusal, AuthorizationCode, AuthorizationTransaction, Callback,
    DEFAULT_TRANSACTION_SECONDS, Grant, ListenerCapabilities, LoopbackHost, LoopbackListener,
    LoopbackRedirect, MAX_REDIRECT_PATH_CHARS, MixUpDefence, RedirectError,
    UnmetListenerRequirement,
};
pub use cursor::{
    CursorError, MAX_SYNC_CURSOR_CHARS, SyncCursor, SyncCursorKind, SyncCursorParts, SyncWindow,
};
pub use diagnostics::{
    DiagnosticField, DiagnosticFinding, DiagnosticSeverity, Redaction, diagnostics_for,
};
pub use health::{
    ConnectorHealth, DEFAULT_FRESHNESS_SECONDS, HealthProbe, HealthSignal, MAX_HEALTH_DETAIL_CHARS,
    ProbeOutcome, ReauthReason,
};
pub use manifest::{
    AuthMethodDeclaration, Classification, CompatibilityStatus, CompatibilityVerdict,
    ConnectorError, ConnectorId, ConnectorManifest, ConnectorOperation, ConnectorVersion,
    DataResidency, DocumentationLinks, LinkKind, MAX_CONNECTOR_OPERATION_ID_CHARS,
    MAX_CONNECTOR_OPERATIONS, MAX_CONNECTOR_SCOPES, MAX_CONNECTOR_SECRET_FIELDS,
    MAX_MANIFEST_LINKS, MIN_SUPPORTED_JARVIS_VERSION, PollingInterval, ProviderIdempotency,
    ResearchRecord, ResidencyVerification, SecretField, SecretKind, ToolEffect, WebhookSupport,
};
pub use ratelimit::{
    BudgetOutcome, MAX_RATE_LIMIT_BURST, MAX_RATE_LIMIT_PER_WINDOW, MAX_RETRY_AFTER_SECONDS,
    RateLimit, RateLimitError, RateLimitScope, RetryClass, RetryDecision, RetryGuidance,
};
pub use readiness::{
    ALL_ITEMS, Attestation, EvidencePath, EvidenceStrength, LiveSmokeTest,
    MAX_ATTESTATION_PURPOSE_CHARS, MAX_ATTESTED_TESTS, MAX_EVIDENCE_PATH_CHARS,
    ReadinessAssessment, ReadinessError, ReadinessGap, ReadinessItem, ReadinessReview,
    describe_live_smoke,
};
pub use scaffold::{
    MAX_SCAFFOLD_NAME_CHARS, SCAFFOLD_VERSION, Scaffold, ScaffoldError, ScaffoldRequest,
    proposed_research_path,
};
pub use token::{
    BEARER_TOKEN_TYPE, MAX_ACCESS_TOKEN_SECONDS, MAX_TOKEN_TYPE_CHARS, RefreshExchange,
    RevocationKind, RevocationOutcome, TokenEndpointFailure, TokenRequestOutcome, TokenResponse,
    TokenSet, split_scope,
};
pub use webhook::{
    MAX_REPLAY_WINDOW_SECONDS, MAX_SIGNATURE_HEADER_CHARS, MAX_TIMESTAMP_SKEW_SECONDS,
    ReplayWindow, SignatureAlgorithm, SignatureEncoding, SignatureError, SignatureScheme,
    WebhookBinding, WebhookDelivery, WebhookRejection,
};
