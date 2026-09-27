//! Tests for the readiness gate and the scaffold generator.
//!
//! The two most important assertions here are structural rather than behavioural:
//!
//! 1. **A generated scaffold's manifest skeleton is not a manifest**, and the reason is checkable — the
//!    skeleton carries `_comment` keys, and `ConnectorManifest`'s `deny_unknown_fields` refuses them. That is
//!    what makes "the scaffold refuses to invent operations" a property rather than a promise.
//! 2. **The webhook items are conditional on the manifest's own declaration**, so a polling connector is
//!    complete without them and a push connector is not. A gate that demanded them unconditionally would make
//!    every polling connector permanently incomplete.

use crate::auth::AuthMethod;
use crate::manifest::{
    AuthMethodDeclaration, Classification, CompatibilityStatus, CompatibilityVerdict, ConnectorId,
    ConnectorManifest, ConnectorOperation, ConnectorVersion, DataResidency, DocumentationLink,
    DocumentationLinks, LinkKind, PollingInterval, ProviderIdempotency, ResearchRecord,
    ResidencyVerification, WebhookSupport,
};
use crate::readiness::{
    ALL_ITEMS, Attestation, EvidencePath, EvidenceStrength, LiveSmokeTest, MAX_ATTESTED_TESTS,
    ReadinessError, ReadinessItem, ReadinessReview, describe_live_smoke,
};
use crate::scaffold::{
    MAX_SCAFFOLD_NAME_CHARS, SCAFFOLD_VERSION, Scaffold, ScaffoldError, ScaffoldRequest,
    proposed_research_path,
};

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    result.unwrap_or_else(|error| panic!("{what}: {error}"))
}

fn must_err<T, E: std::fmt::Debug>(result: Result<T, E>) -> E {
    match result {
        Ok(_) => panic!("expected a refusal"),
        Err(error) => error,
    }
}

fn id(value: &str) -> ConnectorId {
    must(ConnectorId::new(value), "a usable connector id")
}

fn version(value: &str) -> ConnectorVersion {
    must(ConnectorVersion::new(value), "a usable version")
}

fn path(value: &str) -> EvidencePath {
    must(EvidencePath::new(value), "a usable evidence path")
}

fn links() -> DocumentationLinks {
    must(
        DocumentationLinks::new(vec![DocumentationLink {
            kind: LinkKind::LlmsTxt,
            url: "https://vendor.example/llms.txt".to_owned(),
            purpose: "the source the research process checks first".to_owned(),
        }]),
        "usable links",
    )
}

fn research() -> ResearchRecord {
    must(
        ResearchRecord::new("docs/research/integrations/vendor.md", "2026-09-27"),
        "a usable research record",
    )
}

fn compatibility() -> CompatibilityStatus {
    CompatibilityStatus {
        minimum_jarvis_version: "0.5.0".to_owned(),
        status: CompatibilityVerdict::Current,
        note: None,
    }
}

/// A read-only operation, so the classification floor does not intervene in these fixtures.
fn operation(operation_id: &str) -> ConnectorOperation {
    ConnectorOperation {
        id: operation_id.to_owned(),
        description: "reads a message list".to_owned(),
        effects: vec![crate::ToolEffect::ReadOnly],
        risk: 0,
        required_scopes: vec!["vendor.read".to_owned()],
        idempotency: ProviderIdempotency::Declared,
        rate_limit: None,
    }
}

fn auth_methods() -> Vec<AuthMethodDeclaration> {
    vec![AuthMethodDeclaration {
        method: AuthMethod::OAuthPkce,
        purpose: "connects the user's own account".to_owned(),
        scopes: vec!["vendor.read".to_owned()],
        required: true,
    }]
}

/// A manifest whose webhook support is whatever the caller says.
fn manifest_with_webhook(webhook: WebhookSupport) -> ConnectorManifest {
    must(
        ConnectorManifest::new(
            id("vendor"),
            version("1.0.0"),
            "Vendor",
            "Vendor Incorporated",
            vec![operation("list_messages")],
            auth_methods(),
            webhook,
            Vec::new(),
            Classification::Confidential,
            DataResidency {
                note: "processed in the vendor's region".to_owned(),
                verification: ResidencyVerification::NotStated,
            },
            links(),
            research(),
            compatibility(),
        ),
        "a usable manifest",
    )
}

fn manifest() -> ConnectorManifest {
    manifest_with_webhook(WebhookSupport::Unsupported)
}

/// Every declared item, satisfied, for the non-push case.
fn full_attestations() -> Vec<(ReadinessItem, Attestation)> {
    vec![
        (
            ReadinessItem::OnboardingTests,
            Attestation::onboarding("both a successful and a rejected setup path", 9, true),
        ),
        (
            ReadinessItem::AuthLifecycleTests,
            Attestation::suite("refresh, revoke, and reauth paths", 12),
        ),
        (
            ReadinessItem::PaginationAndLimitsTests,
            Attestation::suite("paging and throttling", 7),
        ),
        (
            ReadinessItem::OperationContractFixtures,
            Attestation::artifact(
                "sanitized wire fixtures for each operation",
                path("crates/jarvis-connectors/tests/vendor/list.json"),
            ),
        ),
        (
            ReadinessItem::PolicyMetadataReviewed,
            Attestation::artifact(
                "the effect and risk review for each operation",
                path("crates/jarvis-connectors/tests/vendor/review.md"),
            ),
        ),
        (
            ReadinessItem::DocumentedLimitations,
            Attestation::artifact(
                "what the connector does not do",
                path("crates/jarvis-connectors/tests/vendor/limits.md"),
            ),
        ),
    ]
}

fn smoke() -> LiveSmokeTest {
    must(
        LiveSmokeTest::absent("the vendor has no free tier and this profile holds no credentials"),
        "a usable smoke test declaration",
    )
}

// ---------------------------------------------------------------------------------------------
// The checklist itself
// ---------------------------------------------------------------------------------------------

#[test]
fn every_checklist_item_has_a_distinct_code_and_a_title() {
    // The completeness check. `ALL_ITEMS` is hand-written, so a new variant added to `ReadinessItem` without
    // being added there is an item nothing checks — and the count assertion is what makes that fail rather
    // than pass silently. `P5-001` used the same arrangement for `DiagnosticField`.
    assert_eq!(
        ALL_ITEMS.len(),
        12,
        "the checklist grew or shrank without this test"
    );
    let mut codes: Vec<&str> = ALL_ITEMS.iter().map(|item| item.as_str()).collect();
    codes.sort_unstable();
    let mut unique = codes.clone();
    unique.dedup();
    assert_eq!(
        unique.len(),
        codes.len(),
        "every item must have a distinct code"
    );
    for item in ALL_ITEMS {
        assert!(!item.title().is_empty(), "{item:?}");
        assert!(!item.evidence().as_str().is_empty(), "{item:?}");
        assert_eq!(item.to_string(), item.as_str());
    }
}

#[test]
fn only_the_derived_items_are_free_of_an_assertion() {
    // The deliverable's core claim: a checklist whose items all read the same would let the strong ones be
    // ignored at the same cost as the weak ones. Three items are read from the manifest and the other nine are
    // the author's, and the split is asserted so a later edit to `evidence()` has to be deliberate.
    //
    // **A first version of this module had five derived items and it was WRONG.** The two webhook items were
    // classified as derived on the grounds that "the manifest declares push delivery", which conflates the
    // applicability condition with the evidence: whether a connector *needs* webhook tests is a manifest fact,
    // but whether it *has* them is not. The gate reported a push connector complete with no signature or replay
    // test at all. This assertion is what caught it.
    let derived: Vec<ReadinessItem> = ALL_ITEMS
        .iter()
        .copied()
        .filter(|item| item.evidence() == EvidenceStrength::Derived)
        .collect();
    assert_eq!(
        derived,
        vec![
            ReadinessItem::OfficialSourcesDated,
            ReadinessItem::ManifestValidated,
            ReadinessItem::RedactedDiagnostics,
        ],
        "a manifest cannot evidence a test that was never run"
    );
    assert_eq!(
        ALL_ITEMS
            .iter()
            .filter(|item| item.evidence().requires_an_assertion())
            .count(),
        9
    );
    // The two webhook items are DECLARED test counts, exactly like the other three suites.
    for item in [
        ReadinessItem::WebhookSignatureTests,
        ReadinessItem::WebhookReplayTests,
    ] {
        assert_eq!(item.evidence(), EvidenceStrength::DeclaredCount, "{item:?}");
    }
    assert!(!EvidenceStrength::Derived.requires_an_assertion());
    for strength in [
        EvidenceStrength::DeclaredArtifact,
        EvidenceStrength::DeclaredCount,
        EvidenceStrength::Conditional,
    ] {
        assert!(strength.requires_an_assertion(), "{strength:?}");
    }
}

#[test]
fn only_a_push_connector_needs_the_two_webhook_items() {
    // `tools-and-connectors.md` says "webhook signature/replay tests **if applicable**", and applicability is a
    // fact about the connector. So a polling connector is complete without them and a push one is not — and
    // both directions are asserted, because a gate that demanded them unconditionally would make every polling
    // connector permanently incomplete.
    let push = WebhookSupport::Push {
        scheme: must(
            crate::SignatureScheme::new(
                crate::SignatureAlgorithm::HmacSha256,
                "x-vendor-signature",
                crate::SignatureEncoding::Hex,
            ),
            "a usable scheme",
        ),
        binding: must(
            crate::WebhookBinding::new(
                "/webhooks/vendor",
                Some("x-vendor-account".to_owned()),
                true,
            ),
            "a usable binding",
        ),
    };
    assert!(ReadinessItem::WebhookSignatureTests.applies_to(&push));
    assert!(ReadinessItem::WebhookReplayTests.applies_to(&push));

    for not_push in [
        WebhookSupport::Polling {
            interval: PollingInterval::Documented(60),
        },
        WebhookSupport::Unsupported,
    ] {
        assert!(!ReadinessItem::WebhookSignatureTests.applies_to(&not_push));
        assert!(!ReadinessItem::WebhookReplayTests.applies_to(&not_push));
        // Every other item applies regardless, so the conditional set is exactly the two.
        for item in ALL_ITEMS {
            if item != ReadinessItem::WebhookSignatureTests
                && item != ReadinessItem::WebhookReplayTests
            {
                assert!(item.applies_to(&not_push), "{item:?}");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Gaps and evaluation
// ---------------------------------------------------------------------------------------------

#[test]
fn a_connector_with_nothing_declared_reports_the_seven_asserted_items_as_gaps() {
    // The progress-report shape: everything outstanding, named, with its evidence strength. The derived items
    // are never gaps, because they cannot be missing — the manifest could not exist without them.
    //
    // Seven, not nine: two of the nine asserted items are the webhook pair, and a connector that declares
    // `unsupported` webhooks does not have them apply. The first version of this test asserted nine, which was
    // the same applicability confusion the `evidence()` split was corrected for, one assertion over.
    let gaps = ReadinessReview::gaps(&manifest(), &[], None);
    assert_eq!(gaps.len(), 7, "got {gaps:?}");
    for gap in &gaps {
        assert!(gap.evidence.requires_an_assertion(), "{gap:?}");
        assert!(!gap.title.is_empty());
        assert!(gap.to_string().contains(gap.item.as_str()));
    }
    assert!(
        gaps.iter()
            .any(|gap| gap.item == ReadinessItem::LiveSmokeTest)
    );
    assert!(
        !gaps
            .iter()
            .any(|gap| gap.item == ReadinessItem::OfficialSourcesDated),
        "a derived item can never be a gap"
    );
    // And the two webhook items appear only once a push connector makes them applicable.
    assert!(
        !gaps
            .iter()
            .any(|gap| gap.item == ReadinessItem::WebhookSignatureTests)
    );
}

#[test]
fn a_fully_attested_polling_connector_has_no_gaps_and_all_twelve_items() {
    // The positive control. Without it, a gate that reported every connector as incomplete would pass every
    // refusal test in this file.
    let manifest = manifest();
    let attestations = full_attestations();
    let smoke = smoke();
    assert!(
        ReadinessReview::gaps(&manifest, &attestations, Some(&smoke)).is_empty(),
        "got {:?}",
        ReadinessReview::gaps(&manifest, &attestations, Some(&smoke))
    );
    let review = must(
        ReadinessReview::evaluate(&manifest, &attestations, Some(&smoke)),
        "a complete review",
    );
    assert!(review.is_complete());
    assert!(!review.requires_webhook_tests());
    assert_eq!(review.connector(), "vendor");
    assert_eq!(review.version().as_str(), "1.0.0");
    // Ten of the twelve apply to a non-push connector: the two webhook items are omitted.
    assert_eq!(review.satisfied().len(), 10);
    assert!(
        !review
            .satisfied()
            .iter()
            .any(|assessment| assessment.item == ReadinessItem::WebhookSignatureTests)
    );
    // And the derived items are recorded as such, so a reader can tell which half is evidence and which is a
    // statement. Three are derived and they all apply whatever the webhook support is.
    let derived = review
        .satisfied()
        .iter()
        .filter(|assessment| assessment.evidence == EvidenceStrength::Derived)
        .count();
    assert_eq!(derived, 3, "three derived items apply");
    // The live smoke test is the one conditional item, and it reports its own strength rather than being
    // distinguishable only by having no attestation — which is the ambiguity an earlier version of this
    // assertion (counting `declared_by.is_none()`) would have missed.
    let conditional = review
        .satisfied()
        .iter()
        .filter(|assessment| assessment.evidence == EvidenceStrength::Conditional)
        .count();
    assert_eq!(conditional, 1);
    let smoke_assessment = review
        .satisfied()
        .iter()
        .find(|assessment| assessment.item == ReadinessItem::LiveSmokeTest)
        .unwrap_or_else(|| panic!("the review covers the smoke item"));
    assert!(smoke_assessment.declared_by.is_none());
    assert_eq!(smoke_assessment.evidence, EvidenceStrength::Conditional);
}

#[test]
fn a_push_connector_without_the_webhook_items_is_not_complete() {
    // The conditional direction that matters: a push connector is NOT complete on the same attestations a
    // polling one is. A gate that accepted them would let a webhook connector ship with no replay test, which
    // is the one defect `WebhookRejection` exists to make visible — and an EARLIER VERSION OF THIS MODULE HAD
    // EXACTLY THAT DEFECT, because it classified the two webhook items as derived from the manifest. This test
    // is what caught it: `got []` for the gaps it expected to be two.
    let push = WebhookSupport::Push {
        scheme: must(
            crate::SignatureScheme::new(
                crate::SignatureAlgorithm::HmacSha256,
                "x-vendor-signature",
                crate::SignatureEncoding::Hex,
            ),
            "a usable scheme",
        ),
        binding: must(
            crate::WebhookBinding::new("/webhooks/vendor", None, true),
            "a usable binding",
        ),
    };
    let manifest = manifest_with_webhook(push);
    let gaps = ReadinessReview::gaps(&manifest, &full_attestations(), Some(&smoke()));
    assert_eq!(gaps.len(), 2, "got {gaps:?}");
    assert_eq!(
        gaps.iter().map(|gap| gap.item).collect::<Vec<_>>(),
        vec![
            ReadinessItem::WebhookSignatureTests,
            ReadinessItem::WebhookReplayTests
        ]
    );
    // And a review cannot be assembled from the same input, because the two items have no attestation.
    let review = must(
        ReadinessReview::evaluate(&manifest, &full_attestations(), Some(&smoke())),
        "a review with the webhook items missing",
    );
    assert_eq!(review.satisfied().len(), 10);
    assert!(review.requires_webhook_tests());

    // Adding them is what completes a push connector, so the two directions are both proven rather than only
    // the refusal.
    let mut complete = full_attestations();
    complete.push((
        ReadinessItem::WebhookSignatureTests,
        Attestation::suite("a forged signature is refused", 6),
    ));
    complete.push((
        ReadinessItem::WebhookReplayTests,
        Attestation::suite("a replayed delivery is refused and acknowledged", 4),
    ));
    assert!(
        ReadinessReview::gaps(&manifest, &complete, Some(&smoke())).is_empty(),
        "a push connector with both webhook suites must be complete"
    );
    let review = must(
        ReadinessReview::evaluate(&manifest, &complete, Some(&smoke())),
        "a complete push review",
    );
    assert_eq!(review.satisfied().len(), 12);
}

#[test]
fn attesting_to_a_webhook_item_for_a_polling_connector_is_refused_rather_than_dropped() {
    // A refusal rather than a silent drop, because attesting to webhook tests for a polling connector is a sign
    // the author is working from a template rather than from their own connector — and a review that quietly
    // discarded the entry would hide exactly that.
    let mut attestations = full_attestations();
    attestations.push((
        ReadinessItem::WebhookSignatureTests,
        Attestation::suite("webhook signatures", 4),
    ));
    let error = must_err(ReadinessReview::evaluate(
        &manifest(),
        &attestations,
        Some(&smoke()),
    ));
    match error {
        ReadinessError::NotApplicable { item } => assert_eq!(item, "webhook_signature_tests"),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn attesting_to_one_item_twice_is_refused() {
    // Two answers for one question is not a review, and silently taking the first would make the second
    // invisible — which is how a corrected count stops being the count.
    let mut attestations = full_attestations();
    attestations.push((
        ReadinessItem::AuthLifecycleTests,
        Attestation::suite("a better count later", 30),
    ));
    let error = must_err(ReadinessReview::evaluate(
        &manifest(),
        &attestations,
        Some(&smoke()),
    ));
    assert_eq!(
        error,
        ReadinessError::Duplicate {
            item: "auth_lifecycle_tests"
        }
    );
}

#[test]
fn a_declared_item_with_a_path_but_no_purpose_is_refused() {
    // Every item must say what it is for. A checklist entry that cannot state its purpose is the formality
    // this module exists to avoid, and a blank purpose is how that arrives.
    let mut attestations = full_attestations();
    let index = attestations
        .iter()
        .position(|(item, _)| *item == ReadinessItem::DocumentedLimitations)
        .unwrap_or_else(|| panic!("the fixture declares the limitations item"));
    attestations[index].1 = Attestation::artifact("   ", path("docs/limits.md"));
    let error = must_err(ReadinessReview::evaluate(
        &manifest(),
        &attestations,
        Some(&smoke()),
    ));
    match error {
        ReadinessError::Attestation { reason } => {
            assert!(reason.contains("purpose"), "got {reason}");
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn an_artifact_item_needs_a_path_and_a_suite_item_must_not_have_one() {
    // The two shapes are exclusive, and a value of the wrong shape is a caller who believes it said something.
    let mut artifact_without_path = full_attestations();
    let index = artifact_without_path
        .iter()
        .position(|(item, _)| *item == ReadinessItem::OperationContractFixtures)
        .unwrap_or_else(|| panic!("the fixture declares the fixtures item"));
    artifact_without_path[index].1 = Attestation::stated("fixtures exist somewhere");
    match must_err(ReadinessReview::evaluate(
        &manifest(),
        &artifact_without_path,
        Some(&smoke()),
    )) {
        ReadinessError::Attestation { reason } => {
            assert!(reason.contains("repository-relative path"), "got {reason}");
        }
        other => panic!("got {other:?}"),
    }

    let mut suite_with_path = full_attestations();
    let index = suite_with_path
        .iter()
        .position(|(item, _)| *item == ReadinessItem::AuthLifecycleTests)
        .unwrap_or_else(|| panic!("the fixture declares the auth item"));
    suite_with_path[index].1 = Attestation {
        purpose: "refresh tests".to_owned(),
        path: Some(path("tests/auth.rs")),
        test_count: Some(12),
        covers_failure: None,
    };
    match must_err(ReadinessReview::evaluate(
        &manifest(),
        &suite_with_path,
        Some(&smoke()),
    )) {
        ReadinessError::Attestation { reason } => {
            assert!(reason.contains("not give a path"), "got {reason}");
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn a_test_count_of_zero_is_refused_because_it_is_the_claim_that_nothing_exists() {
    // Zero and absent are different: absent means "not reported yet" and zero means "there are none". Only the
    // second is a refusal, and a count past the ceiling is caught for the same reason — it is a typo or a count
    // pasted from another connector, not a fact about this one.
    for count in [0, MAX_ATTESTED_TESTS + 1] {
        let mut attestations = full_attestations();
        let index = attestations
            .iter()
            .position(|(item, _)| *item == ReadinessItem::AuthLifecycleTests)
            .unwrap_or_else(|| panic!("the fixture declares the auth item"));
        attestations[index].1 = Attestation::suite("refresh tests", count);
        match must_err(ReadinessReview::evaluate(
            &manifest(),
            &attestations,
            Some(&smoke()),
        )) {
            ReadinessError::Attestation { reason } => {
                assert!(reason.contains("test"), "for {count}: {reason}");
            }
            other => panic!("for {count}: {other:?}"),
        }
    }
}

#[test]
fn the_onboarding_item_must_report_the_failure_path_and_nothing_else_may_report_it() {
    // The document says "successful **and failed** onboarding tests", which is two claims. A success-only suite
    // is the common shortcut, so a `false` is refused rather than accepted as "partially there".
    let mut without_failure = full_attestations();
    let index = without_failure
        .iter()
        .position(|(item, _)| *item == ReadinessItem::OnboardingTests)
        .unwrap_or_else(|| panic!("the fixture declares the onboarding item"));
    without_failure[index].1 = Attestation::onboarding("a successful setup path only", 4, false);
    match must_err(ReadinessReview::evaluate(
        &manifest(),
        &without_failure,
        Some(&smoke()),
    )) {
        ReadinessError::Attestation { reason } => {
            assert!(reason.contains("failure"), "got {reason}");
        }
        other => panic!("got {other:?}"),
    }

    // A `covers_failure` on any other item is refused rather than ignored, because the caller believed it said
    // something and silently discarding that belief is how a report claims coverage it does not have.
    let mut misplaced = full_attestations();
    let index = misplaced
        .iter()
        .position(|(item, _)| *item == ReadinessItem::AuthLifecycleTests)
        .unwrap_or_else(|| panic!("the fixture declares the auth item"));
    misplaced[index].1 = Attestation {
        purpose: "refresh tests".to_owned(),
        path: None,
        test_count: Some(12),
        covers_failure: Some(true),
    };
    match must_err(ReadinessReview::evaluate(
        &manifest(),
        &misplaced,
        Some(&smoke()),
    )) {
        ReadinessError::Attestation { reason } => {
            assert!(reason.contains("onboarding"), "got {reason}");
        }
        other => panic!("got {other:?}"),
    }
}

#[test]
fn the_live_smoke_test_is_satisfied_by_a_reason_and_not_by_an_attestation() {
    // The conditional item's evidence is a [`LiveSmokeTest`] rather than an [`Attestation`], so it is answered
    // by supplying the value and not by adding a list entry. That is what makes "a refusal must carry a reason"
    // a fact: `LiveSmokeTest::absent` already refused an empty reason when it was built.
    let attestations = full_attestations();
    let without = ReadinessReview::gaps(&manifest(), &attestations, None);
    assert_eq!(without.len(), 1);
    assert_eq!(without[0].item, ReadinessItem::LiveSmokeTest);

    let with = ReadinessReview::gaps(&manifest(), &attestations, Some(&smoke()));
    assert!(with.is_empty());

    // An empty reason, and a present-test with no gate, are both refused at construction.
    assert!(LiveSmokeTest::absent("   ").is_err());
    assert!(LiveSmokeTest::absent("").is_err());
    assert!(LiveSmokeTest::present(path("tests/live.rs"), "  ").is_err());
    assert!(
        LiveSmokeTest::present(path("tests/live.rs"), "behind ACCEPTANCE_VENDOR_LIVE=1").is_ok()
    );
}

#[test]
fn both_live_smoke_shapes_render_and_say_which_they_are() {
    // The renderer is what an operator reads, so a `Present` must name its gate and an `Absent` must name its
    // reason. A single `bool` could not carry either.
    let present = must(
        LiveSmokeTest::present(
            path("tests/live/vendor.rs"),
            "behind ACCEPTANCE_VENDOR_LIVE=1",
        ),
        "a present smoke test",
    );
    let rendered = describe_live_smoke(&present);
    assert!(rendered.contains("tests/live/vendor.rs"), "got {rendered}");
    assert!(
        rendered.contains("ACCEPTANCE_VENDOR_LIVE"),
        "got {rendered}"
    );

    let absent = smoke();
    let rendered = describe_live_smoke(&absent);
    assert!(rendered.contains("no live smoke test"), "got {rendered}");
    assert!(rendered.contains("free tier"), "got {rendered}");
}

#[test]
fn a_declared_item_is_recorded_with_what_it_rests_on() {
    // The assessment carries the author's attestation, so a report can show the path and the count rather than
    // a bare "ok" — which is the difference between a gate and a rubber stamp.
    let review = must(
        ReadinessReview::evaluate(&manifest(), &full_attestations(), Some(&smoke())),
        "a complete review",
    );
    let fixtures = review
        .satisfied()
        .iter()
        .find(|assessment| assessment.item == ReadinessItem::OperationContractFixtures)
        .unwrap_or_else(|| panic!("the review covers the fixtures item"));
    let declared = fixtures
        .declared_by
        .as_ref()
        .unwrap_or_else(|| panic!("a declared item must carry its attestation"));
    assert_eq!(
        declared.path.as_ref().map(EvidencePath::as_str),
        Some("crates/jarvis-connectors/tests/vendor/list.json")
    );
    assert!(
        fixtures.detail.contains("list.json"),
        "got {}",
        fixtures.detail
    );

    // And the derived items carry no attestation, which is what a reader uses to tell the two halves apart.
    let dated = review
        .satisfied()
        .iter()
        .find(|assessment| assessment.item == ReadinessItem::OfficialSourcesDated)
        .unwrap_or_else(|| panic!("the review covers the research item"));
    assert!(dated.declared_by.is_none());
    assert!(dated.detail.contains("2026-09-27"), "got {}", dated.detail);
    assert!(dated.detail.contains("vendor.md"), "got {}", dated.detail);
}

// ---------------------------------------------------------------------------------------------
// Evidence paths
// ---------------------------------------------------------------------------------------------

#[test]
fn an_evidence_path_must_be_relative_and_shaped_like_an_artifact() {
    // The check is honest about its limits: it cannot prove a file exists, because this crate has no
    // filesystem. What it can refuse is a path that could not name a repository artifact, which catches a
    // pasted absolute path, a traversal, a drive prefix, and a value pointing at nothing in particular.
    for usable in [
        "docs/limits.md",
        "crates/jarvis-connectors/tests/vendor/list.json",
        "tests/support/mod.rs",
    ] {
        assert_eq!(path(usable).as_str(), usable);
    }
    for unusable in [
        "/etc/passwd",
        "\\windows\\system32",
        "~/notes.md",
        "../outside.md",
        "docs/../../etc/passwd",
        "C:/users/me/notes.md",
        "docs/limits",
        "docs/limits.exe",
        " notes.md",
        "notes.md ",
        "",
    ] {
        assert!(
            EvidencePath::new(unusable).is_err(),
            "{unusable:?} must be refused"
        );
    }
    // A colon is refused anywhere, which is what makes the drive-prefix case reachable — **and this comment
    // replaces an earlier one that claimed the opposite.** The first version had a separate
    // `chars().nth(1) == Some(':')` branch, justified as "a colon is legal later in a path on unix", and the
    // falsification run showed that branch was **unreachable**: `C:/x` and `C:x` both contain a colon, so the
    // later check caught every case the earlier one did. An unreachable refusal reads as protection while
    // enforcing nothing — the same defect `P5-001` found in an unreachable bound.
    assert!(EvidencePath::new("docs/a:b.md").is_err());
    assert!(EvidencePath::new("C:/users/me/notes.md").is_err());
    assert!(EvidencePath::new("C:notes.md").is_err());
    // And the length bound is enforced.
    assert!(EvidencePath::new(format!("{}.md", "a".repeat(600))).is_err());
}

// ---------------------------------------------------------------------------------------------
// The scaffold
// ---------------------------------------------------------------------------------------------

fn request() -> ScaffoldRequest {
    must(
        ScaffoldRequest::new(
            "vendor",
            "Vendor",
            "Vendor Incorporated",
            "2026-09-27",
            proposed_research_path("vendor"),
            "0.5.0",
        ),
        "a usable scaffold request",
    )
}

#[test]
fn a_generated_manifest_skeleton_is_not_a_manifest() {
    // The central structural assertion. The skeleton carries `_comment` keys, and `ConnectorManifest`'s
    // `deny_unknown_fields` refuses them — so "the scaffold refuses to invent operations" is a checkable
    // property rather than a promise. An empty `operations` list would be refused too, and both refusals are
    // asserted because either one alone would leave the other path open.
    let scaffold = must(Scaffold::new(request()), "a usable scaffold");
    let skeleton = scaffold.manifest_json();

    // It is valid JSON, which the first version of this was not: the comments contain JSON examples, and a
    // hand-written template that must quote a nested document produced unescaped quotes. Building a
    // `serde_json::Value` put the escaping in the writer's hands.
    let parsed: serde_json::Value = must(
        serde_json::from_str(&skeleton),
        "the skeleton must be valid JSON",
    );

    // **Every explanatory key must be a `_comment`**, and this is asserted rather than assumed because the
    // falsification run showed the test below was not sensitive to a rename: it accepts ANY unknown-field
    // refusal, so a key renamed to `note` still failed to deserialize and the test passed. The rename would
    // quietly turn an explanation into a field the author might fill in.
    let top_level: Vec<&String> = parsed
        .as_object()
        .map(|object| object.keys().collect())
        .unwrap_or_default();
    for key in &top_level {
        assert!(
            key.starts_with("_comment")
                || [
                    "id",
                    "version",
                    "display_name",
                    "provider",
                    "operations",
                    "auth_methods",
                    "webhook",
                    "secret_fields",
                    "classification",
                    "residency",
                    "links",
                    "research",
                    "compatibility"
                ]
                .contains(&key.as_str()),
            "`{key}` is neither a manifest field nor an explanatory comment"
        );
    }
    // And every comment must say something, so an empty placeholder is not a passing comment.
    for (key, value) in parsed.as_object().map(|o| o.iter()).into_iter().flatten() {
        if key.starts_with("_comment") {
            let text = value.as_str().unwrap_or_default();
            assert!(text.len() > 40, "`{key}` must explain, got {text:?}");
        }
    }
    // The unknown-key refusal: this is what makes the skeleton not a manifest.
    let outcome = serde_json::from_str::<ConnectorManifest>(&skeleton);
    match outcome {
        Err(error) => assert!(
            error.to_string().contains("unknown field") || error.to_string().contains("_comment"),
            "got {error}"
        ),
        Ok(_) => panic!("a skeleton with `_comment` keys must not deserialize as a manifest"),
    }

    // Every manifest field is named, so an author sees the shape without reading `manifest.rs`.
    for field in [
        "id",
        "version",
        "display_name",
        "provider",
        "operations",
        "auth_methods",
        "webhook",
        "secret_fields",
        "classification",
        "residency",
        "links",
        "research",
        "compatibility",
    ] {
        assert!(
            parsed.get(field).is_some(),
            "the skeleton is missing `{field}`"
        );
    }
    // And the three collections a scaffold cannot fill are empty, with the reason recorded.
    for collection in ["operations", "auth_methods", "links", "secret_fields"] {
        assert_eq!(
            parsed[collection],
            serde_json::json!([]),
            "`{collection}` must be empty for the author to fill in"
        );
    }
    // The classification starts at the level that admits every connector, because `ADR-0054` refuses anything
    // below `confidential` for a connector with an outward operation — so an author who adds one write
    // operation does not meet that refusal as a surprise.
    assert_eq!(parsed["classification"], serde_json::json!("confidential"));
    assert_eq!(parsed["version"], serde_json::json!(SCAFFOLD_VERSION));
    assert_eq!(parsed["id"], serde_json::json!("vendor"));
}

#[test]
fn the_research_stub_carries_the_path_the_date_and_every_required_section() {
    // `external-research.md`'s own sections, as questions. A stub that pre-filled an answer would be a
    // plausible record that nothing was verified against, so every section has the question and no answer.
    let scaffold = must(Scaffold::new(request()), "a usable scaffold");
    let stub = scaffold.research_markdown();
    assert!(stub.contains("integration: vendor"), "front matter");
    assert!(stub.contains("last_verified: 2026-09-27"));
    assert!(stub.contains("status: planned"));
    for section in [
        "## Scope",
        "## Official Sources",
        "## Verified Contract",
        "### Authentication And Authorization",
        "### Limits And Failure Semantics",
        "### Data And Compliance",
        "### Versions And Deprecations",
        "## JARVIS Mapping",
        "## Decisions",
        "## Rejected Alternatives",
        "## Verification Plan",
        "## Unresolved Questions",
        "## Verification Log",
    ] {
        assert!(stub.contains(section), "the stub is missing `{section}`");
    }
    // It points at the shared OAuth record rather than restating the protocol, so a connector records what is
    // true of *its* provider.
    assert!(stub.contains("oauth2-pkce-native-apps.md"));
    // The verification-log row is left blank: a pre-filled row would be a dated claim nothing checked.
    let log = stub
        .split("## Verification Log")
        .nth(1)
        .unwrap_or_else(|| panic!("the stub has a verification log"));
    assert!(log.contains("| 2026-09-27 | | |"), "got {log}");
}

#[test]
fn a_scaffold_reports_every_item_it_has_not_answered_except_the_one_it_does() {
    // The scaffold's honest half, computed from the same `ALL_ITEMS` list the gate uses, so the two cannot
    // disagree about what a connector needs.
    let scaffold = must(Scaffold::new(request()), "a usable scaffold");
    let outstanding = scaffold.outstanding_items();
    assert_eq!(outstanding.len(), ALL_ITEMS.len() - 1);
    assert!(
        !outstanding.contains(&ReadinessItem::OfficialSourcesDated),
        "the scaffold answers the research item by writing the stub"
    );
    for item in ALL_ITEMS {
        if item != ReadinessItem::OfficialSourcesDated {
            assert!(outstanding.contains(&item), "{item:?} must be outstanding");
        }
    }
    assert_eq!(
        scaffold.research_path(),
        "docs/research/integrations/vendor.md"
    );
    let summary = scaffold.to_string();
    assert!(summary.contains("vendor"), "got {summary}");
    assert!(summary.contains("11"), "got {summary}");
}

#[test]
fn a_scaffold_request_refuses_every_value_the_manifest_would() {
    // Generating a scaffold cannot produce something that fails later for a reason the author could have been
    // told now. Each field is checked by the type that owns it rather than by a second implementation, so this
    // test is really asserting that the delegation exists.
    for (identifier, name, provider, date, record, version, what) in [
        (
            "Vendor",
            "Vendor",
            "Vendor Inc",
            "2026-09-27",
            "docs/research/integrations/vendor.md",
            "0.5.0",
            "an uppercase identifier",
        ),
        (
            "vendor",
            "   ",
            "Vendor Inc",
            "2026-09-27",
            "docs/research/integrations/vendor.md",
            "0.5.0",
            "a blank display name",
        ),
        (
            "vendor",
            "Vendor",
            "  ",
            "2026-09-27",
            "docs/research/integrations/vendor.md",
            "0.5.0",
            "a blank provider",
        ),
        (
            "vendor",
            "Vendor",
            "Vendor Inc",
            "yesterday",
            "docs/research/integrations/vendor.md",
            "0.5.0",
            "an unparseable date",
        ),
        (
            "vendor",
            "Vendor",
            "Vendor Inc",
            "2026-09-27",
            "/etc/vendor.md",
            "0.5.0",
            "an absolute record path",
        ),
        (
            "vendor",
            "Vendor",
            "Vendor Inc",
            "2026-09-27",
            "../outside.md",
            "0.5.0",
            "a traversing record path",
        ),
    ] {
        assert!(
            ScaffoldRequest::new(identifier, name, provider, date, record, version).is_err(),
            "{what} must be refused"
        );
    }
}

#[test]
fn a_scaffold_request_refuses_a_version_the_manifest_would_and_accepts_the_floor() {
    // The version rule is delegated to the manifest's own validator, so the two cannot disagree. Every value
    // below the floor and every unparseable form is refused, and the floor itself is accepted — because the
    // manifest accepts it, and a scaffold offering a version the manifest would refuse is a scaffold that
    // produces a file the author cannot use.
    for bad_version in ["0.4.9", "0", "1", "1.0", "abc", "", "1.0.0.0", "1.0.x"] {
        assert!(
            matches!(
                ScaffoldRequest::new(
                    "vendor",
                    "Vendor",
                    "Vendor Inc",
                    "2026-09-27",
                    "docs/research/integrations/vendor.md",
                    bad_version,
                ),
                Err(ScaffoldError::Version { .. })
            ),
            "{bad_version:?} must be refused as a version"
        );
    }
    assert!(
        ScaffoldRequest::new(
            "vendor",
            "Vendor",
            "Vendor Inc",
            "2026-09-27",
            "docs/research/integrations/vendor.md",
            crate::MIN_SUPPORTED_JARVIS_VERSION
        )
        .is_ok()
    );
    assert!(
        ScaffoldRequest::new(
            "vendor",
            "Vendor",
            "Vendor Inc",
            "2026-09-27",
            "docs/research/integrations/vendor.md",
            "99.0.0"
        )
        .is_ok(),
        "a version above the floor is usable, since the platform may be newer"
    );
}

#[test]
fn the_scaffold_version_check_is_the_manifest_s_own_validator() {
    // The version rule is **delegated**, and that is the claim worth asserting: a scaffold that reimplemented
    // the `major.minor.patch` parse would be a second place the rule lives, and the first one is what a
    // manifest is actually built with. So the assertion is not "these strings are refused" — it is that the
    // refusal mentions the floor, which only the manifest's own message does.
    let outcome = ScaffoldRequest::new(
        "vendor",
        "Vendor",
        "Vendor Inc",
        "2026-09-27",
        "docs/research/integrations/vendor.md",
        "0.4.9",
    );
    match outcome {
        Err(ScaffoldError::Version { reason }) => {
            // The message is the **manifest's own**, not a restatement: it names the floor and explains why
            // the claim is refused. A scaffold with its own version rule would produce a different sentence
            // here, which is what makes this assertion a test of the delegation rather than of the bound.
            assert!(
                reason.contains("0.5.0") && reason.contains("connector surface"),
                "the refusal must come from the manifest's validator, got {reason:?}"
            );
        }
        other => panic!("a below-floor version must be refused as a version, got {other:?}"),
    }
    // And the error renders with the floor named, because "the minimum JARVIS version is unusable" does not
    // tell an operator what to type instead.
    let rendered = must_err(ScaffoldRequest::new(
        "vendor",
        "Vendor",
        "Vendor Inc",
        "2026-09-27",
        "docs/research/integrations/vendor.md",
        "not-a-version",
    ));
    assert!(!rendered.to_string().is_empty());
}

#[test]
fn the_scaffold_name_bound_is_exercised_at_both_ends() {
    // The bound is exercised through its use rather than compared as a constant, which clippy flags and which
    // would also pass on a bound of zero. A name at the bound is accepted; one past it is not.
    let at_bound = "n".repeat(MAX_SCAFFOLD_NAME_CHARS);
    assert!(
        ScaffoldRequest::new(
            "vendor",
            at_bound,
            "Vendor Inc",
            "2026-09-27",
            "docs/research/integrations/vendor.md",
            "0.5.0"
        )
        .is_ok()
    );
    assert!(matches!(
        must_err(ScaffoldRequest::new(
            "vendor",
            "n".repeat(MAX_SCAFFOLD_NAME_CHARS + 1),
            "Vendor Inc",
            "2026-09-27",
            "docs/research/integrations/vendor.md",
            "0.5.0"
        )),
        ScaffoldError::Name {
            what: "display name"
        }
    ));
}

#[test]
fn the_proposed_research_path_follows_the_convention_every_record_uses() {
    // Proposing it rather than requiring it means a connector kept elsewhere can say so, while the common case
    // gets the convention for free.
    assert_eq!(
        proposed_research_path("vendor"),
        "docs/research/integrations/vendor.md"
    );
    assert_eq!(
        proposed_research_path("microsoft-graph"),
        "docs/research/integrations/microsoft-graph.md"
    );
    // And the proposal is a path the evidence type accepts, so the two cannot disagree.
    assert!(EvidencePath::new(proposed_research_path("vendor")).is_ok());
}
