//! Tests for Google's scope categories and the accounting check.
//!
//! # What these tests prove
//!
//! That this crate **correctly reads a hand-transcribed table** and that the accounting **fails closed** on a
//! scope the table does not list. They do **not** prove that Google's categories are these categories: the
//! table was transcribed from the scopes page on 2026-09-27 and a recategorisation would make it stale, which
//! is why the table carries a date and the research record's Verification Plan keeps the "fixture with a date"
//! requirement.
//!
//! The load-bearing cases are the **two directions of the fail-closed rule** and the **exactness of the
//! match**, because each is a place where a plausible implementation under-reports a burden — the direction
//! that lets an unassessed deployment ship.

use super::*;

/// The categories the scopes page published, as the connector declares them for Gmail.
///
/// Built from the production table rather than a second copy, so a test cannot pass against a table the
/// connector does not use.
fn table() -> Vec<(&'static str, ScopeCategory)> {
    crate::google::gmail_scope_categories()
}

#[test]
fn the_three_published_categories_carry_different_burdens() {
    // The page's own definitions, each asserted against the burden it produces. Without this, a `burden()`
    // that returned one value for everything would leave every accounting test below passing.
    assert_eq!(
        ScopeCategory::NonSensitive.burden(),
        VerificationBurden::BasicReview
    );
    assert_eq!(
        ScopeCategory::Sensitive.burden(),
        VerificationBurden::AdditionalReview
    );
    assert_eq!(
        ScopeCategory::Restricted.burden(),
        VerificationBurden::AdditionalReviewAndConditionalAssessment
    );
    // And the three burdens are distinct, so "additional review" cannot silently stand for the restricted
    // case — which is the confusion that would let a restricted scope pass as merely sensitive.
    assert_ne!(
        VerificationBurden::AdditionalReview,
        VerificationBurden::AdditionalReviewAndConditionalAssessment
    );
    assert_ne!(
        VerificationBurden::BasicReview,
        VerificationBurden::AdditionalReview
    );
    // Only the basic review permits shipping without further review.
    assert!(VerificationBurden::BasicReview.permits_basic_review_only());
    assert!(!VerificationBurden::AdditionalReview.permits_basic_review_only());
    assert!(!VerificationBurden::Unestablished.permits_basic_review_only());
}

#[test]
fn the_security_assessment_is_conditional_and_that_is_a_separate_value_from_not_required() {
    // Google's wording is "if you store restricted scope data on servers (or transmit), then you must go
    // through a security assessment" — so the answer for a restricted scope is a **condition**, not a `yes`,
    // and the condition is about the deployment's data handling rather than about the scope.
    assert_eq!(
        ScopeCategory::Restricted.assessment(),
        AssessmentRequirement::IfStoredOrTransmitted
    );
    assert_eq!(
        ScopeCategory::NonSensitive.assessment(),
        AssessmentRequirement::NotRequired
    );
    assert_eq!(
        ScopeCategory::Sensitive.assessment(),
        AssessmentRequirement::NotRequired
    );
    // **The conflation this type exists to prevent.** "No assessment is required" and "it is not established
    // whether one is" are different answers, and a `bool` would make them the same value.
    assert_ne!(
        AssessmentRequirement::NotRequired,
        AssessmentRequirement::NotEstablished
    );
    assert_eq!(
        ScopeCategory::Unknown.assessment(),
        AssessmentRequirement::NotEstablished
    );
    // And the predicate is not "required regardless" but "may be required", because Google's rule is
    // conditional — so a caller with an unknown deployment asks this and fails closed rather than assuming.
    assert!(
        AssessmentRequirement::IfStoredOrTransmitted.may_require_an_assessment(),
        "an unresolved condition must not plan as though no assessment is needed"
    );
    assert!(
        AssessmentRequirement::NotEstablished.may_require_an_assessment(),
        "an unestablished category must not plan as though no assessment is needed"
    );
    assert!(
        !AssessmentRequirement::NotRequired.may_require_an_assessment(),
        "only the positively-excluded case answers false"
    );
}

#[test]
fn an_unlisted_scope_is_not_a_cheap_scope() {
    // **The direction that matters.** A scope Google's table does not list has no established category, and
    // reading it as non-sensitive would let a deployment ship believing it needs only the basic review.
    assert!(!ScopeCategory::Unknown.is_established());
    assert_eq!(
        ScopeCategory::Unknown.burden(),
        VerificationBurden::Unestablished
    );
    assert!(
        !ScopeCategory::Unknown.burden().permits_basic_review_only(),
        "an unaccounted scope must not read as needing only the basic review"
    );
    // Every established category reports itself as established, so `is_established` is not merely false-always.
    for category in [
        ScopeCategory::NonSensitive,
        ScopeCategory::Sensitive,
        ScopeCategory::Restricted,
    ] {
        assert!(category.is_established(), "{category} must be established");
    }
}

#[test]
fn the_connectors_own_scopes_are_accounted_for_and_its_burden_is_the_restricted_one() {
    // **The accounting check the research record's Verification Plan asked for.** It runs against the
    // manifest's real scopes rather than a fixture list, so adding a scope to this connector is what the test
    // reacts to — which is the property that makes "adding a scope forces a decision" true rather than
    // aspirational.
    let manifest = crate::google::GoogleConnector::manifest()
        .unwrap_or_else(|error| panic!("the manifest must build: {error}"));
    let declared: Vec<String> = manifest
        .auth_methods()
        .iter()
        .flat_map(|method| method.scopes.clone())
        .collect();
    assert!(!declared.is_empty(), "the connector requests scopes");

    let accounting = account(&declared, &table());
    let unaccounted = accounting.unaccounted();
    // The two scopes the record does **not** establish a Gmail category for, and neither is a mistake:
    // `calendar.readonly` is a Google API whose category page was not the one read, and `openid` is not a
    // Google API scope at all. Listing them explicitly is the accounting working: each is a decision recorded
    // in the research record (Unresolved Question 2) rather than an assumption folded into a table.
    assert_eq!(
        unaccounted,
        vec![
            "openid",
            "https://www.googleapis.com/auth/calendar.readonly"
        ],
        "the scopes with no recorded category must be listed rather than defaulted"
    );
    // So the deployment's burden is **not** stated, and that is the honest answer: two of its four scopes have
    // no established category, and reporting the restricted maximum of the known two would present a partial
    // reading as a complete one.
    assert!(!accounting.is_complete());
    assert_eq!(accounting.burden(), VerificationBurden::Unestablished);
    assert!(!accounting.burden().permits_basic_review_only());

    // And the Gmail scope it does request is restricted, which is the fact that governs what deploying costs.
    let gmail_read = accounting
        .entries()
        .iter()
        .find(|entry| entry.scope == crate::google::SCOPE_GMAIL_READONLY)
        .unwrap_or_else(|| panic!("the connector requests the Gmail read scope"));
    assert_eq!(gmail_read.category, ScopeCategory::Restricted);
    assert_eq!(
        gmail_read.category.assessment(),
        AssessmentRequirement::IfStoredOrTransmitted,
        "the Gmail read scope carries the conditional assessment"
    );
}

#[test]
fn a_fully_accounted_scope_set_reports_the_heaviest_burden_it_contains() {
    // The positive control for the test above: the same function over scopes that **are** all listed must
    // report a stated burden, or the fail-closed path would pass for a function that never states one.
    let declared = vec![
        "https://www.googleapis.com/auth/gmail.labels".to_owned(),
        "https://www.googleapis.com/auth/gmail.readonly".to_owned(),
    ];
    let accounting = account(&declared, &table());
    assert!(accounting.is_complete());
    assert!(accounting.unaccounted().is_empty());
    assert_eq!(
        accounting.burden(),
        VerificationBurden::AdditionalReviewAndConditionalAssessment,
        "the restricted scope must dominate the non-sensitive one"
    );
    assert!(!accounting.burden().permits_basic_review_only());

    // A set that is entirely non-sensitive is the one case that permits the basic review alone — so
    // `permits_basic_review_only` is reachable, not a predicate that is false for everything.
    let cheap = account(
        &["https://www.googleapis.com/auth/gmail.labels".to_owned()],
        &table(),
    );
    assert_eq!(cheap.burden(), VerificationBurden::BasicReview);
    assert!(cheap.burden().permits_basic_review_only());

    // And a connector declaring no scopes is vacuous rather than unestablished: there is no scope whose
    // burden is missing.
    let empty = account(&[], &table());
    assert!(empty.is_complete());
    assert_eq!(empty.burden(), VerificationBurden::BasicReview);
}

#[test]
fn the_scope_match_is_exact_because_a_prefix_would_inherit_the_wrong_category() {
    // A scope string goes into an authorization request, so it is an identifier rather than prose. A prefix or
    // case-insensitive match would let `gmail.readonly.suffix` inherit `gmail.readonly`'s restricted category —
    // or, worse in the other direction, let a **longer** scope the table does not list inherit a *cheaper*
    // category from a scope that is its prefix.
    let table = table();
    let near = vec![
        // A suffix on the restricted read scope.
        "https://www.googleapis.com/auth/gmail.readonly.suffix".to_owned(),
        // The non-sensitive labels scope with a suffix.
        "https://www.googleapis.com/auth/gmail.labels.suffix".to_owned(),
        // Differing case, which must not match.
        "https://www.googleapis.com/auth/gmail.LABELS".to_owned(),
    ];
    let accounting = account(&near, &table);
    assert!(
        !accounting.is_complete(),
        "no near-miss may inherit a category: {:?}",
        accounting.unaccounted()
    );
    assert_eq!(
        accounting.entries().len(),
        3,
        "every declared scope is accounted for, whether or not it matched"
    );
    for entry in accounting.entries() {
        assert_eq!(entry.category, ScopeCategory::Unknown, "{}", entry.scope);
    }
    // The control: the exact strings **do** match, so the test above is about exactness rather than about a
    // table that matches nothing.
    let exact = account(
        &[
            "https://www.googleapis.com/auth/gmail.readonly".to_owned(),
            "https://www.googleapis.com/auth/gmail.labels".to_owned(),
        ],
        &table,
    );
    assert!(exact.is_complete());
}

#[test]
fn the_recorded_categories_are_the_published_ones() {
    // The three entries that carry the surprises the research record calls out, asserted against the constants
    // rather than only against literals — so a change to a scope constant cannot leave the table stale without
    // failing here.
    let table = table();
    let find = |scope: &str| {
        table
            .iter()
            .find(|(known, _)| *known == scope)
            .map(|(_, category)| *category)
    };
    assert_eq!(
        find(crate::google::SCOPE_GMAIL_READONLY),
        Some(ScopeCategory::Restricted),
        "the scope this connector requests is restricted"
    );
    assert_eq!(
        find("https://www.googleapis.com/auth/gmail.send"),
        Some(ScopeCategory::Sensitive),
        "a send is sensitive rather than restricted"
    );
    assert_eq!(
        find("https://www.googleapis.com/auth/gmail.labels"),
        Some(ScopeCategory::NonSensitive),
        "labels is the one generally useful non-sensitive Gmail scope"
    );
    assert_eq!(
        find("https://www.googleapis.com/auth/gmail.metadata"),
        Some(ScopeCategory::Restricted),
        "metadata is restricted too, which is the counter-intuitive one"
    );
    // `openid` and the Calendar scope are deliberately **absent** from a Gmail table, so the accounting
    // reports them rather than this test asserting a category nobody read.
    assert_eq!(find(crate::google::SCOPE_OPENID), None);
    assert_eq!(find(crate::google::SCOPE_CALENDAR_READONLY), None);
    // The table's date is the record's, so a recategorisation cannot be recorded in one and not the other.
    assert_eq!(
        SCOPE_CATEGORIES_RECORDED_ON,
        crate::google::RESEARCH_VERIFIED_ON
    );
}

#[test]
fn the_category_codes_are_distinct_and_non_empty() {
    // A code table where two variants share a string would make a stored value ambiguous, which is the same
    // check the diagnostics field set carries.
    let codes = [
        ScopeCategory::NonSensitive,
        ScopeCategory::Sensitive,
        ScopeCategory::Restricted,
        ScopeCategory::Unknown,
    ]
    .map(ScopeCategory::as_str);
    for code in codes {
        assert!(!code.is_empty());
    }
    let unique: std::collections::BTreeSet<&str> = codes.iter().copied().collect();
    assert_eq!(unique.len(), codes.len(), "codes must be distinct");

    let burdens = [
        VerificationBurden::BasicReview,
        VerificationBurden::AdditionalReview,
        VerificationBurden::AdditionalReviewAndConditionalAssessment,
        VerificationBurden::Unestablished,
    ]
    .map(VerificationBurden::as_str);
    let unique: std::collections::BTreeSet<&str> = burdens.iter().copied().collect();
    assert_eq!(unique.len(), burdens.len(), "burden codes must be distinct");

    let assessments = [
        AssessmentRequirement::NotRequired,
        AssessmentRequirement::IfStoredOrTransmitted,
        AssessmentRequirement::NotEstablished,
    ]
    .map(AssessmentRequirement::as_str);
    let unique: std::collections::BTreeSet<&str> = assessments.iter().copied().collect();
    assert_eq!(
        unique.len(),
        assessments.len(),
        "assessment codes must be distinct"
    );
}
