//! Tests for the connector manifest and its cross-field rules.
//!
//! These are the tests that decide whether a manifest can be **installed**, so each one is named after the
//! refusal it pins rather than after the code path it covers. The falsification record for this slice is in
//! `TODO.md`.

use super::*;
use crate::manifest::{AuthMethodDeclaration, ConnectorOperation};
use crate::ratelimit::{RateLimitEvidence, RateLimitScope};

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn id(value: &str) -> ConnectorId {
    must(ConnectorId::new(value), "a valid connector identifier")
}

fn version(value: &str) -> ConnectorVersion {
    must(ConnectorVersion::new(value), "a valid version")
}

/// A minimal operation with the given effects and risk.
fn operation(id: &str, effects: Vec<ToolEffect>, risk: u8) -> ConnectorOperation {
    ConnectorOperation {
        id: id.to_owned(),
        description: "reads something".to_owned(),
        effects,
        risk,
        required_scopes: Vec::new(),
        idempotency: ProviderIdempotency::Declared,
        rate_limit: None,
    }
}

/// The documentation links a complete manifest needs.
fn links() -> DocumentationLinks {
    must(
        DocumentationLinks::new(vec![
            DocumentationLink {
                kind: LinkKind::LlmsTxt,
                url: "https://example.invalid/llms.txt".to_owned(),
                purpose: "the vendor's machine-readable documentation index".to_owned(),
            },
            DocumentationLink {
                kind: LinkKind::RateLimits,
                url: "https://example.invalid/quotas".to_owned(),
                purpose: "quota and rate limit documentation".to_owned(),
            },
        ]),
        "valid documentation links",
    )
}

fn research() -> ResearchRecord {
    must(
        ResearchRecord::new("docs/research/integrations/example.md", "2026-09-27"),
        "a valid research record",
    )
}

fn compatibility() -> CompatibilityStatus {
    CompatibilityStatus {
        minimum_jarvis_version: MIN_SUPPORTED_JARVIS_VERSION.to_owned(),
        status: CompatibilityVerdict::Current,
        note: None,
    }
}

fn residency() -> DataResidency {
    DataResidency {
        note: "processed by the provider in the EU".to_owned(),
        verification: ResidencyVerification::Verified,
    }
}

fn auth_methods() -> Vec<AuthMethodDeclaration> {
    vec![AuthMethodDeclaration {
        method: AuthMethod::OAuthPkce,
        purpose: "connect a mailbox".to_owned(),
        scopes: vec!["https://example.invalid/auth/mail.readonly".to_owned()],
        required: true,
    }]
}

/// A complete, valid manifest with the given operations.
fn manifest(operations: Vec<ConnectorOperation>) -> Result<ConnectorManifest, ConnectorError> {
    ConnectorManifest::new(
        id("example"),
        version("1.0.0"),
        "Example",
        "Example Incorporated",
        operations,
        auth_methods(),
        WebhookSupport::Unsupported,
        Vec::new(),
        Classification::Confidential,
        residency(),
        links(),
        research(),
        compatibility(),
    )
}

#[test]
fn a_complete_manifest_is_accepted_and_derives_its_high_water_marks() {
    let read = operation("list_messages", vec![ToolEffect::ReadOnly], 0);
    let send = operation(
        "send_message",
        vec![ToolEffect::Write, ToolEffect::ExternalCommunication],
        2,
    );
    let manifest = must(manifest(vec![read, send]), "a complete manifest");
    assert_eq!(manifest.id().as_str(), "example");
    assert_eq!(manifest.operations().len(), 2);
    // The high-water mark is what a grant decision asks, so it is derived rather than stored: a stored copy
    // could disagree with the operations it summarizes.
    assert_eq!(manifest.highest_risk(), 2);
    // The effect floor is the operations' own maximum, and the risk must be at least it. Asserted as a
    // comparison rather than as a literal so the invariant is what is pinned.
    assert!(
        manifest.highest_risk() >= manifest.effect_floor(),
        "the highest declared risk must be at least the effect floor"
    );
    assert_eq!(manifest.classification(), Classification::Confidential);
}

#[test]
fn an_operation_may_not_survive_a_missing_effect_set() {
    // The central rule `P3-001` records: a derived `Deserialize` would make an empty effect set
    // representable, and an empty set means "this does nothing" — which is not a statement a provider call
    // can make. A manifest carrying one is refused at parse rather than installed as an empty capability.
    match manifest(vec![operation("list_messages", Vec::new(), 0)]) {
        Err(ConnectorError::Operations { reason }) => {
            assert!(reason.contains("at least one effect class"), "got {reason}");
        }
        other => panic!("an operation with no effects must be refused, got {other:?}"),
    }
}

#[test]
fn an_operation_below_its_effect_floor_is_refused_and_the_floor_is_named() {
    // `P3-001`: risk >= the effect floor, "context can raise risk but cannot lower a hard policy floor". A
    // manifest is the FIRST place an operation's risk is stated, so an under-declared operation cannot be
    // installed at all — rather than being installed and refused at registration, which would report the
    // refusal as a platform problem.
    let outcome = manifest(vec![operation(
        "send_message",
        vec![ToolEffect::ExternalCommunication],
        0,
    )]);
    match outcome {
        Err(ConnectorError::RiskBelowFloor {
            operation,
            declared,
            floor,
        }) => {
            assert_eq!(operation, "send_message");
            assert_eq!(declared, 0);
            // External communication's floor is 2, asserted against the type rather than a literal so the
            // two cannot drift.
            let expected_floor = must(
                EffectSet::new(vec![ToolEffect::ExternalCommunication])
                    .ok_or("external communication is a non-empty effect set"),
                "the effect set",
            )
            .risk_floor();
            assert_eq!(floor, expected_floor);
            // The message must name both numbers, because "risk is too low" without them sends the author
            // hunting the table.
            let message = ConnectorError::RiskBelowFloor {
                operation,
                declared,
                floor,
            }
            .to_string();
            assert!(message.contains("at least 2"), "got {message}");
        }
        other => panic!("an under-declared operation must be refused, got {other:?}"),
    }
}

#[test]
fn an_operation_above_the_platform_maximum_is_refused() {
    let outcome = manifest(vec![operation(
        "send_message",
        vec![ToolEffect::ExternalCommunication],
        jarvis_tools::MAX_RISK_LEVEL + 1,
    )]);
    match outcome {
        Err(ConnectorError::RiskAboveMaximum {
            declared, maximum, ..
        }) => {
            assert_eq!(declared, jarvis_tools::MAX_RISK_LEVEL + 1);
            assert_eq!(maximum, jarvis_tools::MAX_RISK_LEVEL);
        }
        other => panic!("a risk above the maximum must be refused, got {other:?}"),
    }
}

#[test]
fn a_connector_with_no_operations_is_refused() {
    // A connector with nothing to do is a configuration that installs cleanly and then cannot be called;
    // `P3-002`'s "empty surface is an error" applies one layer earlier.
    assert!(matches!(
        manifest(Vec::new()),
        Err(ConnectorError::Operations { .. })
    ));
}

#[test]
fn two_operations_may_not_share_an_identifier() {
    // Two operations with one name make a call reach whichever was registered last — the same argument
    // `jarvis-mcp`'s `NameAssignments` records, where the fix was to key on the pair rather than the value.
    let first = operation("list_messages", vec![ToolEffect::ReadOnly], 0);
    let second = operation("list_messages", vec![ToolEffect::ReadOnly], 0);
    match manifest(vec![first, second]) {
        Err(ConnectorError::Operations { reason }) => {
            assert!(reason.contains("unique"), "got {reason}");
        }
        other => panic!("a duplicate operation identifier must be refused, got {other:?}"),
    }
}

#[test]
fn an_operation_identifier_that_could_never_be_a_tool_name_segment_is_refused() {
    // The identifier becomes a tool name segment, so an uppercase or dotted one would be a connector that
    // parses and installs and whose operations can never be registered. `jarvis-mcp`'s `ServableName` had to
    // work around exactly this for MCP tool names; here the identifier is refused instead.
    for unusable in [
        "ListMessages",
        "list.messages",
        "list-messages",
        "list messages",
        "",
    ] {
        let outcome = manifest(vec![operation(unusable, vec![ToolEffect::ReadOnly], 0)]);
        assert!(
            matches!(outcome, Err(ConnectorError::Operations { .. })),
            "`{unusable}` must be refused as an operation identifier, got {outcome:?}"
        );
    }
    // And an underscore-named one is accepted, so the alphabet is not over-restricted.
    assert!(
        manifest(vec![operation(
            "list_messages",
            vec![ToolEffect::ReadOnly],
            0
        )])
        .is_ok()
    );
}

#[test]
fn at_least_one_auth_method_must_be_required() {
    // A connector whose methods are all optional has no way to connect, and a `bool` per method read
    // independently would let that state through.
    let optional_only = vec![AuthMethodDeclaration {
        method: AuthMethod::ApiKey,
        purpose: "an alternative".to_owned(),
        scopes: Vec::new(),
        required: false,
    }];
    let outcome = ConnectorManifest::new(
        id("example"),
        version("1.0.0"),
        "Example",
        "Example Incorporated",
        vec![operation("list_messages", vec![ToolEffect::ReadOnly], 0)],
        optional_only,
        WebhookSupport::Unsupported,
        Vec::new(),
        Classification::Internal,
        residency(),
        links(),
        research(),
        compatibility(),
    );
    match outcome {
        Err(ConnectorError::Auth { reason }) => {
            assert!(reason.contains("required"), "got {reason}");
        }
        other => panic!("an all-optional auth list must be refused, got {other:?}"),
    }
}

#[test]
fn the_same_auth_method_may_not_be_declared_twice() {
    let duplicated = vec![
        AuthMethodDeclaration {
            method: AuthMethod::ApiKey,
            purpose: "first".to_owned(),
            scopes: Vec::new(),
            required: true,
        },
        AuthMethodDeclaration {
            method: AuthMethod::ApiKey,
            purpose: "second".to_owned(),
            scopes: Vec::new(),
            required: false,
        },
    ];
    let outcome = ConnectorManifest::new(
        id("example"),
        version("1.0.0"),
        "Example",
        "Example Incorporated",
        vec![operation("list_messages", vec![ToolEffect::ReadOnly], 0)],
        duplicated,
        WebhookSupport::Unsupported,
        Vec::new(),
        Classification::Internal,
        residency(),
        links(),
        research(),
        compatibility(),
    );
    match outcome {
        Err(ConnectorError::Auth { reason }) => {
            assert!(reason.contains("only once"), "got {reason}");
        }
        other => panic!("a duplicate auth method must be refused, got {other:?}"),
    }
}

#[test]
fn documentation_without_a_vendor_contract_is_refused() {
    // The research obligation: `external-research.md` requires the vendor's own contract, and a manifest
    // whose links are all rate-limit or terms pages has not named one. A homepage is deliberately not enough,
    // which is what `LinkKind::satisfies_research_requirement` decides.
    for kind in [
        LinkKind::RateLimits,
        LinkKind::Terms,
        LinkKind::Authentication,
    ] {
        let outcome = DocumentationLinks::new(vec![DocumentationLink {
            kind,
            url: "https://example.invalid/page".to_owned(),
            purpose: "one aspect of the vendor's behaviour".to_owned(),
        }]);
        match outcome {
            Err(ConnectorError::Documentation { reason }) => {
                assert!(reason.contains("cannot substitute"), "got {reason}");
            }
            other => panic!("a {kind:?}-only link set must be refused, got {other:?}"),
        }
    }
    // And a vendor contract link satisfies it, so the rule is a requirement rather than a blanket refusal.
    for kind in [
        LinkKind::LlmsTxt,
        LinkKind::Documentation,
        LinkKind::Specification,
        LinkKind::Sdk,
    ] {
        assert!(
            DocumentationLinks::new(vec![DocumentationLink {
                kind,
                url: "https://example.invalid/vendor".to_owned(),
                purpose: "the vendor's own contract".to_owned(),
            }])
            .is_ok(),
            "{kind:?} must satisfy the research requirement"
        );
    }
}

#[test]
fn a_documentation_link_must_be_https_with_a_stated_purpose() {
    // A manifest's links are followed by a browser, and the document explains what a credential grant
    // authorizes — so `http` is a document that can be changed in transit.
    for url in [
        "http://example.invalid/docs",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "example.invalid/docs",
    ] {
        let outcome = DocumentationLinks::new(vec![DocumentationLink {
            kind: LinkKind::Documentation,
            url: url.to_owned(),
            purpose: "the vendor's documentation".to_owned(),
        }]);
        assert!(
            matches!(outcome, Err(ConnectorError::Documentation { .. })),
            "`{url}` must be refused as a documentation URL, got {outcome:?}"
        );
    }
    // A bare URL with no purpose leaves the reader guessing which question it answers.
    let outcome = DocumentationLinks::new(vec![DocumentationLink {
        kind: LinkKind::Documentation,
        url: "https://example.invalid/docs".to_owned(),
        purpose: "   ".to_owned(),
    }]);
    assert!(matches!(outcome, Err(ConnectorError::Documentation { .. })));
}

#[test]
fn a_documentation_kind_may_appear_only_once() {
    // A second entry of the same kind would silently override the first, which is how a manifest ends up
    // pointing at a page nobody chose.
    let outcome = DocumentationLinks::new(vec![
        DocumentationLink {
            kind: LinkKind::Documentation,
            url: "https://example.invalid/a".to_owned(),
            purpose: "first".to_owned(),
        },
        DocumentationLink {
            kind: LinkKind::Documentation,
            url: "https://example.invalid/b".to_owned(),
            purpose: "second".to_owned(),
        },
    ]);
    match outcome {
        Err(ConnectorError::Documentation { reason }) => {
            assert!(reason.contains("silently override"), "got {reason}");
        }
        other => panic!("a duplicate link kind must be refused, got {other:?}"),
    }
}

#[test]
fn a_research_record_needs_a_repository_relative_path_and_a_real_date() {
    // The record is the pointer that lets a connector be re-verified, so a path that escapes the repository
    // or a date that is not a date makes the obligation unmeetable.
    for (path, date) in [
        ("/etc/passwd", "2026-09-27"),
        ("../../secrets.md", "2026-09-27"),
        ("C:\\notes.md", "2026-09-27"),
        ("", "2026-09-27"),
        ("docs/research/x.md", "2026-9-7"),
        ("docs/research/x.md", "yesterday"),
        ("docs/research/x.md", ""),
    ] {
        assert!(
            ResearchRecord::new(path, date).is_err(),
            "`{path}` / `{date}` must be refused"
        );
    }
    assert!(ResearchRecord::new("docs/research/integrations/google.md", "2026-09-27").is_ok());
}

#[test]
fn a_compatibility_claim_below_the_connector_surface_is_refused() {
    // `P5-001` adds the first connector contract, so a claim of compatibility with an older version is a
    // claim about a platform that had no connector surface at all.
    let below = CompatibilityStatus {
        minimum_jarvis_version: "0.4.9".to_owned(),
        status: CompatibilityVerdict::Current,
        note: None,
    };
    match ConnectorManifest::new(
        id("example"),
        version("1.0.0"),
        "Example",
        "Example Incorporated",
        vec![operation("list_messages", vec![ToolEffect::ReadOnly], 0)],
        auth_methods(),
        WebhookSupport::Unsupported,
        Vec::new(),
        Classification::Internal,
        residency(),
        links(),
        research(),
        below,
    ) {
        Err(ConnectorError::Compatibility { reason }) => {
            assert!(
                reason.contains("0.5.0"),
                "the refusal must name the floor: {reason}"
            );
        }
        other => panic!("a claim below the connector surface must be refused, got {other:?}"),
    }
    // The bound itself is accepted, so the check is a boundary rather than an off-by-one.
    assert!(
        manifest(vec![operation(
            "list_messages",
            vec![ToolEffect::ReadOnly],
            0
        )])
        .is_ok(),
        "the floor itself must be accepted"
    );
    // A malformed version is refused too, because `0.5` and a paragraph are both text and neither is a
    // `major.minor.patch` triple.
    for unusable in ["0.5", "1", "a.b.c", "", "1.2.3.4"] {
        let declaration = CompatibilityStatus {
            minimum_jarvis_version: unusable.to_owned(),
            status: CompatibilityVerdict::Current,
            note: None,
        };
        let outcome = ConnectorManifest::new(
            id("example"),
            version("1.0.0"),
            "Example",
            "Example Incorporated",
            vec![operation("list_messages", vec![ToolEffect::ReadOnly], 0)],
            auth_methods(),
            WebhookSupport::Unsupported,
            Vec::new(),
            Classification::Internal,
            residency(),
            links(),
            research(),
            declaration,
        );
        assert!(
            matches!(outcome, Err(ConnectorError::Compatibility { .. })),
            "`{unusable}` must be refused as a minimum version, got {outcome:?}"
        );
    }
}

#[test]
fn a_non_current_compatibility_verdict_must_say_what_is_known() {
    // "Not verified" with no explanation is indistinguishable from an author who forgot, and
    // `security.md`'s "missing or stale evidence fails closed" is why the permissive verdict is the narrow
    // one.
    for (status, note, expected_ok) in [
        (
            CompatibilityVerdict::Unverified,
            Some("not yet tested against 0.5.1"),
            true,
        ),
        (CompatibilityVerdict::Unverified, None, false),
        (CompatibilityVerdict::Unverified, Some("   "), false),
        (
            CompatibilityVerdict::VendorChange,
            Some("the vendor renamed the endpoint"),
            true,
        ),
        (CompatibilityVerdict::Incompatible, None, false),
        // `Current` needs no note, because there is nothing to explain.
        (CompatibilityVerdict::Current, None, true),
    ] {
        let declaration = CompatibilityStatus {
            minimum_jarvis_version: MIN_SUPPORTED_JARVIS_VERSION.to_owned(),
            status,
            note: note.map(str::to_owned),
        };
        let outcome = ConnectorManifest::new(
            id("example"),
            version("1.0.0"),
            "Example",
            "Example Incorporated",
            vec![operation("list_messages", vec![ToolEffect::ReadOnly], 0)],
            auth_methods(),
            WebhookSupport::Unsupported,
            Vec::new(),
            Classification::Internal,
            residency(),
            links(),
            research(),
            declaration,
        );
        assert_eq!(
            outcome.is_ok(),
            expected_ok,
            "{status:?} with note {note:?} should {} but got {outcome:?}",
            if expected_ok {
                "be accepted"
            } else {
                "be refused"
            }
        );
        // The permissive verdict is the narrow one: only `Current` may be installed.
        if status != CompatibilityVerdict::Current {
            assert!(
                !status.is_installable(),
                "{status:?} must not be installable"
            );
        }
    }
    assert!(CompatibilityVerdict::Current.is_installable());
}

#[test]
fn a_secret_field_never_holds_a_value() {
    // The mechanical form of "no token material in model context, URLs/logs, diagnostics, or normal database
    // columns": the type has no value field, so a manifest cannot carry one. This asserts the shape rather
    // than the behaviour, because the absence IS the control.
    let field = SecretField {
        name: "client_secret".to_owned(),
        kind: SecretKind::ClientSecret,
        required: true,
        purpose: "issued by the provider's developer console".to_owned(),
    };
    let encoded = must(serde_json::to_string(&field), "serialize a secret field");
    // The assertion is on the **keys** rather than on the text, because a legitimate value contains the word
    // `secret` — the first version of this test searched the whole document and failed on its own fixture,
    // which is `P4-006`'s lesson about a test whose premise shares the code's vocabulary.
    let parsed: serde_json::Value = must(serde_json::from_str(&encoded), "parse the secret field");
    let object = parsed
        .as_object()
        .unwrap_or_else(|| panic!("a secret field serializes to an object"));
    let keys: Vec<&str> = object.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        vec!["kind", "name", "purpose", "required"],
        "a secret field's serialized form must offer exactly these keys, and no key able to hold a value"
    );
    for forbidden in ["value", "secret", "token", "password", "credential"] {
        assert!(
            !object.contains_key(forbidden),
            "a serialized secret field must not offer a `{forbidden}` key: {encoded}"
        );
    }
    // And the kinds are distinguishable, because each changes how a client handles the value.
    assert!(SecretKind::ClientSecret.is_presented());
    assert!(
        !SecretKind::SigningSecret.is_presented(),
        "a signing secret is compared rather than sent to the provider"
    );
    // Every kind has a distinct code, so a stored field cannot be ambiguous about what it expects.
    let kinds = [
        SecretKind::ClientSecret,
        SecretKind::PersonalAccessToken,
        SecretKind::ApiKey,
        SecretKind::RefreshToken,
        SecretKind::SigningSecret,
        SecretKind::PrivateKey,
        SecretKind::PairingCode,
    ];
    let codes: Vec<&str> = kinds.iter().map(|kind| kind.as_str()).collect();
    let mut unique = codes.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        codes.len(),
        "the secret kind codes must be distinct"
    );
    // Exactly one kind is compared rather than presented, which is the property diagnostics branch on.
    let compared: Vec<SecretKind> = kinds
        .into_iter()
        .filter(|kind| !kind.is_presented())
        .collect();
    assert_eq!(compared, vec![SecretKind::SigningSecret]);
}

#[test]
fn secret_field_names_must_be_unique_and_lowercase() {
    let duplicated = vec![
        SecretField {
            name: "client_secret".to_owned(),
            kind: SecretKind::ClientSecret,
            required: true,
            purpose: "first".to_owned(),
        },
        SecretField {
            name: "client_secret".to_owned(),
            kind: SecretKind::ApiKey,
            required: false,
            purpose: "second".to_owned(),
        },
    ];
    let outcome = ConnectorManifest::new(
        id("example"),
        version("1.0.0"),
        "Example",
        "Example Incorporated",
        vec![operation("list_messages", vec![ToolEffect::ReadOnly], 0)],
        auth_methods(),
        WebhookSupport::Unsupported,
        duplicated,
        Classification::Secret,
        residency(),
        links(),
        research(),
        compatibility(),
    );
    match outcome {
        Err(ConnectorError::Secrets { reason }) => {
            assert!(reason.contains("unique"), "got {reason}");
        }
        other => panic!("a duplicate secret field name must be refused, got {other:?}"),
    }
    // An uppercase name is refused, because a configuration key's case is not something to be unsure about.
    let uppercase = vec![SecretField {
        name: "ClientSecret".to_owned(),
        kind: SecretKind::ClientSecret,
        required: true,
        purpose: "issued by the provider".to_owned(),
    }];
    let outcome = ConnectorManifest::new(
        id("example"),
        version("1.0.0"),
        "Example",
        "Example Incorporated",
        vec![operation("list_messages", vec![ToolEffect::ReadOnly], 0)],
        auth_methods(),
        WebhookSupport::Unsupported,
        uppercase,
        Classification::Secret,
        residency(),
        links(),
        research(),
        compatibility(),
    );
    assert!(matches!(outcome, Err(ConnectorError::Secrets { .. })));
}

#[test]
fn a_connector_identifier_that_could_never_be_a_namespace_is_refused() {
    // The identifier becomes a tool namespace, so an unusable one would be a connector whose operations
    // cannot be registered — and a leading or doubled separator produces an empty namespace segment.
    for unusable in [
        "",
        "-",
        "_",
        "-leading",
        "trailing-",
        "a--b",
        "UpperCase",
        "dots.not.allowed",
        "sp ace",
    ] {
        assert!(
            ConnectorId::new(unusable).is_err(),
            "`{unusable}` must be refused as a connector identifier"
        );
    }
    for usable in ["google", "microsoft-graph", "home_assistant", "github2"] {
        assert!(
            ConnectorId::new(usable).is_ok(),
            "`{usable}` must be accepted as a connector identifier"
        );
    }
}

#[test]
fn a_connector_version_may_not_carry_a_control_character() {
    // A version reaches logs, and a newline in a log field forges a line.
    for unusable in ["", "1.0.0\n", "1.0.0\r\n", "a\u{0}b"] {
        assert!(
            ConnectorVersion::new(unusable).is_err(),
            "`{}` must be refused as a version",
            unusable.escape_debug()
        );
    }
    assert!(ConnectorVersion::new("2026.09.27+build.1").is_ok());
}

#[test]
fn the_classification_mirrors_the_sensitivity_vocabulary() {
    // A connector's classification becomes a `jarvis_core::Sensitivity` when its output enters a context, so
    // two spellings for one level would be a privacy bug rather than a cosmetic one. This asserts the mirror
    // against the domain type rather than against a copy of its strings.
    for (classification, expected) in [
        (Classification::Public, jarvis_core::Sensitivity::Public),
        (Classification::Internal, jarvis_core::Sensitivity::Internal),
        (
            Classification::Confidential,
            jarvis_core::Sensitivity::Confidential,
        ),
        (Classification::Secret, jarvis_core::Sensitivity::Restricted),
        (
            Classification::Restricted,
            jarvis_core::Sensitivity::Restricted,
        ),
    ] {
        // The same three-level prefix is what makes the comparison meaningful; `Secret` maps to `Restricted`
        // because the domain's own vocabulary has no `Secret` level, and mapping the two to one value is the
        // over-blocking direction.
        let name = classification.as_str();
        let expected_name = expected.as_str();
        let agrees = name == expected_name
            || (classification == Classification::Secret && expected_name == "restricted");
        assert!(
            agrees,
            "{classification:?} (`{name}`) must agree with {expected:?} (`{expected_name}`)"
        );
        // And the remote-model rule must agree with the domain's own flow rule.
        assert_eq!(
            classification.may_reach_a_remote_model(),
            expected.can_flow_to(jarvis_core::Sensitivity::Internal),
            "{classification:?} disagrees with the domain about reaching a remote model"
        );
    }
}

#[test]
fn the_manifest_round_trips_through_json_and_is_validated_on_the_way_back() {
    // `Deserialize` is routed through the constructor, so a manifest loaded from a document is validated
    // exactly as one built in code. Asserting the round trip AND the refusal is what makes the routing real:
    // a type whose `Deserialize` skipped validation would pass the round trip and fail the refusal.
    let manifest = must(
        manifest(vec![operation(
            "list_messages",
            vec![ToolEffect::ReadOnly],
            0,
        )]),
        "a complete manifest",
    );
    let encoded = must(serde_json::to_string(&manifest), "serialize a manifest");
    let decoded: ConnectorManifest = must(serde_json::from_str(&encoded), "deserialize a manifest");
    assert_eq!(decoded, manifest);

    // A document that violates a cross-field rule must be refused at parse. The under-declared risk is the
    // one to use, because it is the rule a naive `derive(Deserialize)` would skip.
    let tampered = encoded.replace(
        "\"effects\":[\"read_only\"]",
        "\"effects\":[\"external_communication\"]",
    );
    assert_ne!(tampered, encoded, "the fixture must actually differ");
    let outcome: Result<ConnectorManifest, _> = serde_json::from_str(&tampered);
    match outcome {
        Err(error) => assert!(
            error.to_string().contains("risk"),
            "the refusal must name the risk, got {error}"
        ),
        Ok(value) => {
            panic!("a manifest with an under-declared risk must be refused, got {value:?}")
        }
    }
}

#[test]
fn an_unknown_manifest_key_is_refused_rather_than_ignored() {
    // A typo in a key is a value the author believes is configured. `deny_unknown_fields` is what makes it a
    // refusal instead of a manifest that loads and behaves as though the field were absent — the same rule
    // `apps/jarvisd`'s config schema applies.
    let manifest = must(
        manifest(vec![operation(
            "list_messages",
            vec![ToolEffect::ReadOnly],
            0,
        )]),
        "a complete manifest",
    );
    let encoded = must(serde_json::to_string(&manifest), "serialize a manifest");
    let tampered = encoded.replacen('{', "{\"unexpected_key\": 1,", 1);
    let outcome: Result<ConnectorManifest, _> = serde_json::from_str(&tampered);
    match outcome {
        Err(error) => assert!(
            error.to_string().contains("unexpected_key"),
            "the refusal must name the unknown key, got {error}"
        ),
        Ok(value) => panic!("an unknown manifest key must be refused, got {value:?}"),
    }
}

#[test]
fn the_high_water_marks_are_derived_from_the_operations_they_summarize() {
    // A stored high-water mark could disagree with the operations it summarizes, so it is computed. This
    // asserts the computation against a set whose members differ, because a set where every operation had
    // the same risk would pass for any implementation.
    let quiet = operation("list_messages", vec![ToolEffect::ReadOnly], 0);
    let moderate = operation("create_draft", vec![ToolEffect::Write], 1);
    let loud = operation(
        "send_message",
        vec![ToolEffect::Write, ToolEffect::ExternalCommunication],
        2,
    );
    let loud_manifest = must(manifest(vec![loud, quiet, moderate]), "a manifest");
    assert_eq!(loud_manifest.highest_risk(), 2);
    assert_eq!(loud_manifest.effect_floor(), 2);
    // Ordering must not matter, since `Vec` order is the document's and not a ranking.
    assert_eq!(
        loud_manifest.highest_risk(),
        must(
            manifest(vec![
                operation("list_messages", vec![ToolEffect::ReadOnly], 0),
                operation("create_draft", vec![ToolEffect::Write], 1),
                operation(
                    "send_message",
                    vec![ToolEffect::Write, ToolEffect::ExternalCommunication],
                    2
                ),
            ]),
            "a reordered manifest"
        )
        .highest_risk()
    );
}

#[test]
fn a_provider_idempotency_claim_decides_whether_a_retry_is_permitted() {
    // The three-valued type is the whole reason a retry question has an answer: `Unknown` refuses, because
    // the question a retry asks is "is a second effect impossible" and an unanswered question must not be
    // read as a yes.
    assert!(ProviderIdempotency::Declared.permits_automatic_retry());
    assert!(ProviderIdempotency::ProviderKey.permits_automatic_retry());
    assert!(!ProviderIdempotency::Unknown.permits_automatic_retry());
    assert!(
        !ProviderIdempotency::NotIdempotent.permits_automatic_retry(),
        "a provider that documents duplication must never be retried automatically"
    );
    // The default is the restrictive one, because a default is what a deserializer fills in.
    assert_eq!(ProviderIdempotency::default(), ProviderIdempotency::Unknown);
}

#[test]
fn a_connector_may_declare_a_rate_limit_and_it_must_be_usable() {
    let with_limit = ConnectorOperation {
        id: "list_messages".to_owned(),
        description: "reads messages".to_owned(),
        effects: vec![ToolEffect::ReadOnly],
        risk: 0,
        required_scopes: Vec::new(),
        idempotency: ProviderIdempotency::Declared,
        rate_limit: Some(must(
            RateLimit::new(
                250,
                60,
                50,
                RateLimitScope::PerUser,
                RateLimitEvidence::Documented,
            ),
            "a valid rate limit",
        )),
    };
    let manifest = must(manifest(vec![with_limit]), "a manifest with a rate limit");
    let declared = manifest.operations()[0]
        .rate_limit()
        .unwrap_or_else(|| panic!("the rate limit must survive validation"));
    // 250 per 60 seconds rounds DOWN to 4 per second, and the direction is deliberate: a rate that
    // under-estimates keeps a caller inside the provider's limit.
    assert_eq!(declared.sustained_per_second(), 4);
    assert!(declared.admits_burst(50));
    assert!(!declared.admits_burst(51));
    assert!(declared.evidence.is_plannable());
    assert!(!RateLimitEvidence::Observed.is_plannable());
}

#[test]
fn a_classification_below_what_the_operations_do_is_refused() {
    // The under-reporting direction, which is the one that matters: a connector declaring `Public` or
    // `Internal` while an operation communicates externally would have its content treated as safe for a
    // remote model. `Classification::may_reach_a_remote_model` is the rule that makes this concrete.
    let outward = operation(
        "send_message",
        vec![ToolEffect::Write, ToolEffect::ExternalCommunication],
        2,
    );
    for too_low in [Classification::Public, Classification::Internal] {
        assert!(
            too_low.may_reach_a_remote_model(),
            "the fixture must use a level that may reach a remote model"
        );
        let outcome = ConnectorManifest::new(
            id("example"),
            version("1.0.0"),
            "Example",
            "Example Incorporated",
            vec![outward.clone()],
            auth_methods(),
            WebhookSupport::Unsupported,
            Vec::new(),
            too_low,
            residency(),
            links(),
            research(),
            compatibility(),
        );
        match outcome {
            Err(ConnectorError::Classification { declared, reason }) => {
                assert_eq!(declared, too_low.as_str());
                assert!(reason.contains("confidential"), "got {reason}");
            }
            other => panic!("{too_low:?} must be refused for an outward operation, got {other:?}"),
        }
    }
    // Every level at or above `Confidential` is accepted, so the rule is a floor rather than an equality: a
    // mail connector legitimately declares `Confidential` regardless of its effects, because what it HANDLES
    // is confidential.
    for high_enough in [
        Classification::Confidential,
        Classification::Secret,
        Classification::Restricted,
    ] {
        assert!(
            ConnectorManifest::new(
                id("example"),
                version("1.0.0"),
                "Example",
                "Example Incorporated",
                vec![outward.clone()],
                auth_methods(),
                WebhookSupport::Unsupported,
                Vec::new(),
                high_enough,
                residency(),
                links(),
                research(),
                compatibility(),
            )
            .is_ok(),
            "{high_enough:?} must be accepted for an outward operation"
        );
    }
    // And a read-only connector may declare `Internal`, so the rule does not force every connector to the top
    // of the ladder — which would make the classification meaningless.
    assert!(
        ConnectorManifest::new(
            id("example"),
            version("1.0.0"),
            "Example",
            "Example Incorporated",
            vec![operation("list_messages", vec![ToolEffect::ReadOnly], 0)],
            auth_methods(),
            WebhookSupport::Unsupported,
            Vec::new(),
            Classification::Internal,
            residency(),
            links(),
            research(),
            compatibility(),
        )
        .is_ok(),
        "a read-only connector must be allowed a low classification"
    );
}

/// A valid webhook binding for the tests: an absolute path and an account header.
fn binding() -> crate::webhook::WebhookBinding {
    must(
        crate::webhook::WebhookBinding::new(
            "/webhooks/example",
            Some("x-account".to_owned()),
            false,
        ),
        "a valid webhook binding",
    )
}

#[test]
fn a_push_declaration_that_verifies_nothing_is_refused() {
    // The guard this pins was **documented and not enforced**. `SignatureAlgorithm::None`'s doc said the value
    // "is refused by `WebhookBinding::new`", but that constructor validates the binding — the path and the
    // account field — and never sees the algorithm, which is a sibling field of the same enum variant. So a
    // manifest could declare `push` with an authenticator of `none` and be accepted, leaving an endpoint that
    // applies unauthenticated writes. A mutation that deleted the branch reproduced the original acceptance,
    // which is how the gap was confirmed rather than assumed.
    let refused = ConnectorManifest::new(
        id("example"),
        version("1.0.0"),
        "Example",
        "Example Incorporated",
        vec![operation("list_messages", vec![ToolEffect::ReadOnly], 0)],
        auth_methods(),
        WebhookSupport::Push {
            scheme: must(
                crate::SignatureScheme::new(
                    crate::SignatureAlgorithm::None,
                    "x-signature",
                    crate::SignatureEncoding::Hex,
                ),
                "a well-formed header",
            ),
            binding: binding(),
        },
        Vec::new(),
        Classification::Confidential,
        residency(),
        links(),
        research(),
        compatibility(),
    );
    match refused {
        Err(ConnectorError::Webhook { reason }) => {
            assert!(
                reason.contains("polling"),
                "the refusal must name the honest alternative: {reason}"
            );
        }
        other => panic!("a push declaration with no authenticator must be refused, got {other:?}"),
    }
}

#[test]
fn a_push_declaration_that_verifies_something_is_accepted() {
    // The other direction, because a rule that refused every push declaration would pass the test above while
    // making the `push` capability unusable — the shape of an over-broad guard.
    for algorithm in [
        crate::SignatureAlgorithm::HmacSha256,
        crate::SignatureAlgorithm::HmacSha1,
        crate::SignatureAlgorithm::Ed25519,
    ] {
        assert!(
            ConnectorManifest::new(
                id("example"),
                version("1.0.0"),
                "Example",
                "Example Incorporated",
                vec![operation("list_messages", vec![ToolEffect::ReadOnly], 0)],
                auth_methods(),
                WebhookSupport::Push {
                    scheme: must(
                        crate::SignatureScheme::new(
                            algorithm,
                            "x-signature",
                            crate::SignatureEncoding::Hex,
                        ),
                        "a well-formed header",
                    ),
                    binding: binding(),
                },
                Vec::new(),
                Classification::Confidential,
                residency(),
                links(),
                research(),
                compatibility(),
            )
            .is_ok(),
            "{algorithm:?} is an authenticator, so a push declaration must be accepted"
        );
    }
    // And `polling` needs no scheme at all, which is what the refusal tells an author to declare instead.
    assert!(
        ConnectorManifest::new(
            id("example"),
            version("1.0.0"),
            "Example",
            "Example Incorporated",
            vec![operation("list_messages", vec![ToolEffect::ReadOnly], 0)],
            auth_methods(),
            WebhookSupport::Polling {
                minimum_interval_seconds: 60
            },
            Vec::new(),
            Classification::Confidential,
            residency(),
            links(),
            research(),
            compatibility(),
        )
        .is_ok(),
        "polling carries no scheme, so it must not be dragged into the refusal"
    );
}
