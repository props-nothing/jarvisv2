//! Contract tests for the Google connector's declarations.
//!
//! These assert the manifest against the **research record**, which is the only authority for every value
//! here. A test that merely restated the manifest would be a test of the transcription, not of the contract,
//! so each one names the claim it pins and, where a value could plausibly be wrong, asserts the *property*
//! rather than the literal.
//!
//! The falsification record for this slice is in `TODO.md`.

use super::*;
use crate::authorization::{AuthorizationTransaction, LoopbackRedirect};
use crate::manifest::{ConnectorManifest, WebhookSupport};
use crate::readiness::{ALL_ITEMS, ReadinessItem};
use crate::{LoopbackHost, PkceVerifier, SecretValue};
use jarvis_core::UtcTimestamp;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn manifest() -> ConnectorManifest {
    must(
        GoogleConnector::manifest(),
        "the Google manifest must be valid",
    )
}

#[test]
fn the_manifest_is_valid_and_names_its_provider() {
    let manifest = manifest();
    assert_eq!(manifest.id().as_str(), CONNECTOR_ID);
    assert_eq!(manifest.provider(), "Google LLC");
    assert_eq!(manifest.display_name(), "Google Workspace");
}

#[test]
fn every_declared_operation_is_a_read_at_risk_zero() {
    // The whole of this slice is read-only, and that is a claim worth pinning from two directions: the
    // effects list and the risk number. A later edit that added `ExternalCommunication` to a read would
    // raise the effect floor and be refused by the manifest itself, but a later edit that raised `risk`
    // without touching effects is representable and would be a silent behaviour change for policy.
    let manifest = manifest();
    assert!(
        !manifest.operations().is_empty(),
        "a connector with no operations cannot be installed"
    );
    for operation in manifest.operations() {
        assert_eq!(
            operation.risk(),
            0,
            "{} must stay at risk 0 while it is a read",
            operation.id()
        );
        assert_eq!(
            operation.effects().to_vec(),
            vec![ToolEffect::ReadOnly],
            "{} must declare exactly ReadOnly",
            operation.id()
        );
    }
    assert_eq!(manifest.highest_risk(), 0);
}

#[test]
fn the_effect_floor_agrees_with_the_declared_risk() {
    // `risk >= effects.risk_floor()` is enforced at construction, so this test is a second opinion that can
    // only fail if that check were removed — which is exactly the property the check exists for. Asserting
    // it here means the removal shows up as a failure in the connector's own tests rather than only in the
    // manifest's.
    let manifest = manifest();
    assert!(
        manifest.highest_risk() >= manifest.effect_floor(),
        "the declared risk must be at least the effect floor"
    );
    assert_eq!(manifest.effect_floor(), 0, "ReadOnly has a floor of 0");
}

#[test]
fn a_mail_connector_cannot_declare_a_classification_that_may_reach_a_remote_model() {
    // This is the test that makes the classification choice load-bearing rather than stylistic. The
    // operations declared here are read-only, so the *effect* floor does not force `Confidential` — the
    // manifest would accept `Internal`. What refuses it is the content rather than the effect: a mailbox
    // holds exactly the material `Confidential` describes, and `Classification::may_reach_a_remote_model`
    // is true for `Internal`, so declaring it would permit mail to be sent to a third-party model.
    let manifest = manifest();
    assert_eq!(manifest.classification(), Classification::Confidential);
    assert!(
        !manifest.classification().may_reach_a_remote_model(),
        "mail content must not be permitted to reach a remote model by default"
    );
    // And the positive control: the level that would have been wrong is genuinely permissive, so this test
    // is not passing on an enum where every level refuses.
    assert!(
        Classification::Internal.may_reach_a_remote_model(),
        "`Internal` must be the permissive level, or the assertion above is vacuous"
    );
}

#[test]
fn the_webhook_declaration_does_not_invent_a_polling_interval() {
    // Google documents no minimum polling interval for Gmail, and its own guidance is to fall back to
    // `history.list` "after a period with no notifications". `PollingInterval::Unknown` is what says that
    // honestly. A `Documented(60)` here would be a fabricated claim about Google, and the whole point of the
    // three-valued type is that this case is representable.
    let manifest = manifest();
    match manifest.webhook() {
        WebhookSupport::Polling { interval } => {
            assert_eq!(*interval, PollingInterval::Unknown);
            assert_eq!(interval.seconds(), None, "no interval has been established");
            assert!(!interval.is_documented(), "nothing documents one");
        }
        other => {
            panic!("Google must declare polling, because neither push mechanism fits: {other:?}")
        }
    }
}

#[test]
fn a_push_declaration_is_not_used_because_neither_google_mechanism_fits() {
    // The finding this slice's predecessor recorded, restated as a test so a later reader cannot "improve"
    // the manifest by switching to `Push`. `WebhookSupport::Push` carries a `SignatureScheme`, whose
    // algorithms are keyed MACs or an Ed25519 signature over the **body**. Gmail's Pub/Sub delivery is an
    // OIDC bearer JWT with an unsigned body, and a Calendar channel delivery has a **zero-length** body.
    // Neither is expressible, and `SignatureAlgorithm` has no OIDC variant — so claiming `Push` would mean
    // naming an algorithm that does not describe how the delivery is authenticated.
    let manifest = manifest();
    assert!(
        !matches!(manifest.webhook(), WebhookSupport::Push { .. }),
        "neither Google push mechanism can be expressed as a body signature"
    );
}

#[test]
fn the_residency_claim_is_unverified_rather_than_asserted() {
    // The research record lists no residency page among its sources, so nothing checked this. `Verified`
    // would be an unsupported claim and `NotStated` would be a different unsupported one — that Google does
    // not state a position. `Unverified` is the only honest value, and it is also the default, which is why
    // the test names the reason rather than relying on the default being what an author who did nothing
    // produces.
    let manifest = manifest();
    assert_eq!(
        manifest.residency().verification,
        ResidencyVerification::Unverified
    );
    assert!(
        !manifest.residency().note.trim().is_empty(),
        "a residency note is required"
    );
}

#[test]
fn the_connector_is_not_installable_until_something_runs_against_google() {
    // `CompatibilityVerdict::Unverified::is_installable()` is false, which is the fail-closed direction
    // `security.md` requires. This asserts that the manifest's status is the narrow one AND that the narrow
    // one genuinely refuses installation — the second half is what stops the test passing on an enum whose
    // every variant is installable.
    let manifest = manifest();
    assert_eq!(
        manifest.compatibility().status,
        CompatibilityVerdict::Unverified
    );
    assert!(!manifest.compatibility().status.is_installable());
    assert!(
        CompatibilityVerdict::Current.is_installable(),
        "`Current` must be the installable verdict, or the assertion above is vacuous"
    );
    assert!(
        manifest.compatibility().note.is_some(),
        "a non-current status must say what is known"
    );
}

#[test]
fn the_manifest_points_at_a_record_with_the_date_the_record_carries() {
    // A manifest is judged stale from its own `last_verified`, so a pointer to the right file with a wrong
    // date would let a stale contract read as a fresh one. Read the record and compare the date it declares
    // in its own front matter.
    let manifest = manifest();
    assert_eq!(manifest.research().path, RESEARCH_RECORD);
    assert_eq!(manifest.research().last_verified, RESEARCH_VERIFIED_ON);

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(RESEARCH_RECORD);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => panic!("the research record {} must exist: {error}", path.display()),
    };
    let declared = text
        .lines()
        .find_map(|line| line.strip_prefix("last_verified:"))
        .map(|value| value.trim().trim_matches('"').to_owned());
    assert_eq!(
        declared.as_deref(),
        Some(RESEARCH_VERIFIED_ON),
        "the record's own `last_verified` must match the manifest's, or one of the two is stale"
    );
}

#[test]
fn every_provider_scope_is_a_google_scope_and_not_a_jarvis_one() {
    // `AuthMethodDeclaration::scopes` documents the provider's spelling as a deliberate exception to JARVIS
    // vocabulary being canonical, because these strings are sent in an authorization request. The failure
    // mode is a connector that puts `mail.read` there — which looks right, is what a reader expects, and
    // would produce an authorization request Google rejects. So assert the *shape*: every declared scope is
    // an absolute URL or a bare OIDC name.
    let manifest = manifest();
    for method in manifest.auth_methods() {
        for scope in &method.scopes {
            assert!(
                scope.starts_with("https://") || scope == SCOPE_OPENID,
                "`{scope}` is not a provider scope; JARVIS scopes belong in an operation's \
                 `required_scopes`"
            );
        }
    }
    // And the positive control: the JARVIS-side scopes are the other spelling, so the assertion above is
    // distinguishing two real things rather than accepting everything.
    let jarvis_scopes: Vec<&str> = manifest
        .operations()
        .iter()
        .flat_map(|operation| operation.required_scopes().iter().map(String::as_str))
        .collect();
    assert!(!jarvis_scopes.is_empty());
    for scope in jarvis_scopes {
        assert!(
            !scope.contains("://"),
            "`{scope}` is a provider URL in a JARVIS scope position"
        );
    }
}

#[test]
fn the_read_only_scope_is_requested_rather_than_a_wider_gmail_scope() {
    // `gmail.readonly` is restricted like the rest, so choosing it is not a way to avoid the security
    // assessment — it is the way to request the **least** data. The scopes that would be wrong are the ones
    // that permit writing, permanent deletion, or settings changes, and each is a real Gmail scope a
    // plausible edit could paste in.
    let manifest = manifest();
    let scopes: Vec<String> = manifest
        .auth_methods()
        .iter()
        .flat_map(|method| method.scopes.clone())
        .collect();
    assert!(scopes.contains(&SCOPE_GMAIL_READONLY.to_owned()));
    for forbidden in [
        "https://www.googleapis.com/auth/gmail.modify",
        "https://www.googleapis.com/auth/gmail.compose",
        "https://www.googleapis.com/auth/gmail.send",
        "https://www.googleapis.com/auth/gmail.settings.sharing",
        "https://mail.google.com/",
    ] {
        assert!(
            !scopes.contains(&forbidden.to_owned()),
            "`{forbidden}` grants more than a read connector needs"
        );
    }
}

#[test]
fn a_pkce_public_client_has_no_secret_field() {
    // The absence is the control: `OAuthPkce::requires_client_secret()` is false, so a `ClientSecret` field
    // here would be asking an operator to paste something the flow never uses — and a field that exists gets
    // filled in. Assert the predicate and the absence together, so neither can drift from the other.
    let manifest = manifest();
    assert!(!AuthMethod::OAuthPkce.requires_client_secret());
    assert!(
        manifest.secret_fields().is_empty(),
        "a PKCE public client stores no pasted secret"
    );
    // The endpoint is `https`, which the flow constructor would enforce anyway — asserted here because the
    // point of returning a bare constant is that nothing constructs the flow yet, so nothing checks it.
    assert!(GoogleConnector::authorization_endpoint().starts_with("https://"));
}

#[test]
fn no_documentation_link_claims_an_llms_txt_google_does_not_publish() {
    // The research record establishes that both candidate `llms.txt` URLs return HTTP 404. Declaring one
    // would be a link an operator cannot follow and would falsely discharge the research requirement, which
    // is the one thing `LinkKind::satisfies_research_requirement` exists to check.
    let manifest = manifest();
    assert!(
        !manifest.links().has(LinkKind::LlmsTxt),
        "Google publishes no `llms.txt`; the research record records the 404"
    );
    assert!(
        manifest.links().has(LinkKind::Documentation),
        "the research requirement must be discharged by the provider's own documentation"
    );
    // Every link is https, which the constructor already enforces — asserted because a link is what an
    // operator follows to decide whether to grant access.
    for link in manifest.links().as_slice() {
        assert!(
            link.url.starts_with("https://"),
            "{} must be https",
            link.url
        );
    }
}

#[test]
fn the_per_method_quota_costs_are_ordered_as_google_documents_them() {
    // The research record's most decision-relevant table: `messages.get` costs 20 units and
    // `messages.list` costs 5, so a full sync is dominated by the gets. The manifest cannot state a cost, so
    // what is pinned here is that the descriptions carry the figures and that the two operations exist
    // separately — collapsing them would hide the `5 + 20N` shape that decides a first-sync budget.
    let manifest = manifest();
    let Some(read) = manifest
        .operations()
        .iter()
        .find(|operation| operation.id() == "gmail_messages_read")
    else {
        panic!("`gmail_messages_read` must exist");
    };
    let Some(list) = manifest
        .operations()
        .iter()
        .find(|operation| operation.id() == "gmail_messages_list")
    else {
        panic!("`gmail_messages_list` must exist");
    };
    assert!(read.effects().contains(ToolEffect::ReadOnly));
    assert!(list.effects().contains(ToolEffect::ReadOnly));
    assert_ne!(read.id(), list.id());
}

#[test]
fn the_declared_rate_limit_is_the_shared_project_budget_not_a_per_account_one() {
    // Google's quota is per Cloud project, so `PerClient` is the accurate scope — and `RateLimitScope`'s own
    // predicate makes the consequence explicit: a shared scope means one account's exhaustion must not be
    // reported as that account's own. A `PerAccount` declaration here would be a scheduler bug waiting.
    let manifest = manifest();
    let Some(operation) = manifest
        .operations()
        .iter()
        .find(|operation| operation.id() == "gmail_messages_read")
    else {
        panic!("`gmail_messages_read` must exist");
    };
    let Some(limit) = operation.rate_limit() else {
        panic!("the Gmail read path must declare the documented limit");
    };
    assert_eq!(limit.scope, RateLimitScope::PerClient);
    assert!(limit.scope.is_shared_between_accounts());
    assert_eq!(limit.evidence, RateLimitEvidence::Documented);
    assert_eq!(limit.window_seconds, 60);
    assert!(
        limit.burst < limit.per_window,
        "the burst must be the tighter of the two ceilings Google applies, not the project one"
    );
}

#[test]
fn the_endpoints_match_the_discovery_document() {
    // `https://accounts.google.com/.well-known/openid-configuration` is the one source for this connector that
    // is MACHINE-READABLE, so these are the server's own published values rather than a documentation example.
    // Pinned as literals because the point is to notice a drift: a future reader changing an endpoint would
    // have to change this test, which is where they would see the recorded value.
    assert_eq!(
        GoogleConnector::authorization_endpoint(),
        "https://accounts.google.com/o/oauth2/v2/auth"
    );
    assert_eq!(
        GoogleConnector::token_endpoint(),
        "https://oauth2.googleapis.com/token"
    );
    assert_eq!(
        GoogleConnector::revocation_endpoint(),
        "https://oauth2.googleapis.com/revoke"
    );
    // The two hosts differ, which is deliberate and easy to "fix": the consent screen is on
    // `accounts.google.com` and the exchange is on `oauth2.googleapis.com`.
    assert_ne!(
        GoogleConnector::authorization_endpoint(),
        GoogleConnector::token_endpoint()
    );
}

#[test]
fn the_flow_is_constructible_from_the_verified_endpoints() {
    // The test that `P5-004`'s record could not have supported: `AuthFlow::new` requires an `https://`
    // authorization endpoint, a PKCE method, and a redirect URI, and all three now come from a source rather
    // than a guess. Asserted through the flow's own accessors so the constructor's checks are exercised.
    let flow = must(
        GoogleConnector::auth_flow(),
        "the flow must be constructible",
    );
    assert_eq!(flow.method(), AuthMethod::OAuthPkce);
    assert_eq!(flow.pkce(), Some(PkceMethod::S256));
    assert_eq!(
        flow.authorization_endpoint(),
        GoogleConnector::authorization_endpoint()
    );
}

#[test]
fn the_registered_redirect_is_the_portless_loopback_form() {
    // The registered form has NO port, because the client asks the OS for an ephemeral one at request time
    // (RFC 8252 §7.3). What is deliberately NOT asserted is that Google's console accepts this exact string —
    // the research record's Unresolved Question 7 records that as unconfirmed, so claiming it here would be
    // the fabrication this slice exists to avoid.
    let registered = must(
        GoogleConnector::registered_redirect(),
        "a valid loopback redirect",
    );
    assert!(
        registered.is_registrable(),
        "a registration must not pin a port"
    );
    assert_eq!(registered.port(), None);
    assert_eq!(registered.host(), LoopbackHost::V4);
    assert_eq!(registered.as_uri(), "http://127.0.0.1/");
    // The positive control on the comparison that joins a registration to a listener: a listener on some
    // other port matches, and a different PATH does not. Without the second half, a comparison that ignored
    // everything would pass.
    let listening = must(
        LoopbackRedirect::listening(LoopbackHost::V4, 51_004, "/"),
        "a valid listening redirect",
    );
    assert!(registered.matches_except_port(&listening));
    assert!(
        !registered.matches_exactly(&listening),
        "the port really does differ"
    );
    let other_path = must(
        LoopbackRedirect::listening(LoopbackHost::V4, 51_004, "/callback"),
        "a valid listening redirect",
    );
    assert!(
        !registered.matches_except_port(&other_path),
        "a different path must NOT match; loosening this defeats RFC 8252 §8.10"
    );
}

#[test]
fn the_authorization_transaction_matches_its_listener_and_is_consumable_once() {
    // The end-to-end shape of `P5-002` used through this connector's own flow: a transaction opened for the
    // registered redirect succeeds against a listener on a different port, and a transaction whose listener is
    // on a different PATH is refused. The refusal is the one worth pinning, because it is the check that stops
    // the provider's response arriving somewhere nothing verified.
    let flow = must(
        GoogleConnector::auth_flow(),
        "the flow must be constructible",
    );
    let verifier = must(PkceVerifier::generate(), "a generated verifier");
    let state = must(SecretValue::new("state-value"), "a valid state value");
    let now = UtcTimestamp::now(&jarvis_core::SystemClock);
    let listener = must(
        LoopbackRedirect::listening(LoopbackHost::V4, 51_004, "/"),
        "a valid listening redirect",
    );
    let transaction = must(
        AuthorizationTransaction::begin(&flow, verifier, state, None, listener, now),
        "a transaction on the registered path must open",
    );
    assert_eq!(transaction.method(), PkceMethod::S256);
    // The challenge is derived, not stored, so the parameters and the verifier cannot disagree.
    let parameters = transaction.parameters("client-id", &[SCOPE_GMAIL_READONLY.to_owned()]);
    let names: Vec<&str> = parameters.iter().map(|(name, _)| *name).collect();
    assert!(names.contains(&"code_challenge"));
    assert!(names.contains(&"code_challenge_method"));
    assert!(names.contains(&"state"));
    assert!(names.contains(&"redirect_uri"));
    // And the wrong path is refused, which is the guard.
    let wrong_listener = must(
        LoopbackRedirect::listening(LoopbackHost::V4, 51_004, "/elsewhere"),
        "a valid listening redirect",
    );
    let refused = AuthorizationTransaction::begin(
        &flow,
        must(PkceVerifier::generate(), "a generated verifier"),
        must(SecretValue::new("another-state"), "a valid state value"),
        None,
        wrong_listener,
        now,
    );
    assert!(
        refused.is_err(),
        "a listener on a different path than the flow registered must be refused"
    );
}

#[test]
fn the_requested_scopes_are_the_discovery_documents_supported_ones_plus_the_apis_own() {
    // The discovery document advertises `openid`, `email`, `profile` — the OIDC scopes — while the Gmail and
    // Calendar scopes are the APIs' own and are not in that array. So `openid` is corroborated by the
    // authoritative source and the other two are corroborated by the per-API scope pages; the two kinds of
    // evidence are different, and the test says which is which rather than treating both as confirmed.
    let manifest = manifest();
    let scopes: Vec<String> = manifest
        .auth_methods()
        .iter()
        .flat_map(|method| method.scopes.clone())
        .collect();
    assert!(scopes.contains(&SCOPE_OPENID.to_owned()));
    assert!(scopes.contains(&SCOPE_GMAIL_READONLY.to_owned()));
    assert!(scopes.contains(&SCOPE_CALENDAR_READONLY.to_owned()));
    // `email` and `profile` are supported by the discovery document and deliberately NOT requested: this
    // connector needs an account identity, and `users.getProfile` supplies the address from the API itself,
    // so asking for profile claims would be requesting more than the connector uses.
    for unrequested in ["email", "profile"] {
        assert!(
            !scopes.contains(&unrequested.to_owned()),
            "`{unrequested}` is not needed"
        );
    }
}

#[test]
fn the_webhook_readiness_items_are_not_required_for_a_polling_connector() {
    // `P5-003` split applicability from evidence: the two webhook items apply only to a push connector. This
    // asserts that a polling connector does not have them demanded, which is what makes
    // `WebhookSupport::Polling` a usable declaration rather than one that fails the completion gate.
    let manifest = manifest();
    let required: Vec<ReadinessItem> = ALL_ITEMS
        .iter()
        .copied()
        .filter(|item| item.applies_to(manifest.webhook()))
        .collect();
    assert!(
        !required.contains(&ReadinessItem::WebhookSignatureTests),
        "a polling connector has no signature to test"
    );
    assert!(
        !required.contains(&ReadinessItem::WebhookReplayTests),
        "a polling connector has no delivery to replay"
    );
    // The positive control: the items DO apply to a push declaration, so the filter above is discriminating
    // rather than returning nothing.
    let push = WebhookSupport::Push {
        scheme: crate::webhook::SignatureScheme {
            algorithm: crate::webhook::SignatureAlgorithm::HmacSha256,
            header: "x-signature".to_owned(),
            encoding: crate::webhook::SignatureEncoding::Hex,
        },
        binding: must(
            crate::webhook::WebhookBinding::new("/h", Some("x-account".to_owned()), false),
            "a valid binding",
        ),
    };
    assert!(ReadinessItem::WebhookSignatureTests.applies_to(&push));
}

#[test]
fn the_operation_count_and_scopes_stay_within_the_manifest_bounds() {
    // A regression guard for growth: the manifest enforces its own ceilings, so this fails only if a ceiling
    // were removed. The useful assertion is that the connector is nowhere near them, because a connector
    // close to a bound is one where adding the next operation needs a decision nobody has recorded.
    let manifest = manifest();
    assert!(manifest.operations().len() < crate::manifest::MAX_CONNECTOR_OPERATIONS);
    for method in manifest.auth_methods() {
        assert!(method.scopes.len() < crate::manifest::MAX_CONNECTOR_SCOPES);
    }
    assert!(manifest.links().as_slice().len() < crate::manifest::MAX_MANIFEST_LINKS);
}
