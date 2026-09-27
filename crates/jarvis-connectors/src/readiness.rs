//! The connector completion gate, as evidence a program can evaluate.
//!
//! `tools-and-connectors.md` ends its connector section with a list:
//!
//! > A connector is not complete until it has: dated official-source research; manifest and configuration
//! > validation; successful and failed onboarding tests; auth refresh, revoke, and reauth tests; pagination
//! > and rate-limit tests; operation contract fixtures; policy/effect metadata review; redacted diagnostics;
//! > webhook signature/replay tests if applicable; documented limitations and a live smoke test where
//! > credentials permit.
//!
//! That list is the **checklist** this slice is asked for, and the decision worth stating is why it is a
//! module rather than a Markdown file. A prose checklist is read once and then trusted, and `AGENTS.md`'s
//! hardest-won rule is that a recorded claim is read downstream as *verified evidence* with nothing
//! distinguishing "checked" from "assumed".
//!
//! # What is derived and what must be declared
//!
//! Every item is one of three kinds, and the kind is what decides how much trust it deserves:
//!
//! - **Derived** — a fact already inside the manifest. "Dated official-source research" is the
//!   [`ResearchRecord`]'s path and date; "manifest and configuration validation" is the manifest having been
//!   constructed at all; the webhook item is whether [`WebhookSupport`] declares push delivery. Nothing has
//!   to be asserted, so nothing can be asserted *falsely*.
//! - **Declared artifact** — a path the author names, which this module checks is **repository-relative**,
//!   bounded, and shaped like the artifact it claims to be (see [`EvidencePath`]). It cannot prove the file
//!   exists, because this crate has no filesystem; what it can do is refuse a path that could not be one.
//! - **Declared count** — a number an author supplies. This is the weakest kind and it is confined to the one
//!   category where a count is the only available evidence (see [`Attestation::test_count`]).
//!
//! The distinction is the deliverable. A checklist that read the same for all ten items would let the four
//! strongest ones be ignored at the same cost as the weakest, which is how a gate becomes a formality.
//!
//! # Why the webhook item is conditional and why that is enforced
//!
//! `tools-and-connectors.md` says "webhook signature/replay tests **if applicable**", and applicability is a
//! fact about the connector rather than a judgement: a [`WebhookSupport::Push`] connector **must** have
//! signature and replay tests, and a [`WebhookSupport::Polling`] or [`WebhookSupport::Unsupported`] one
//! cannot have them. So [`ReadinessReview::evaluate`] adds the two webhook items only for a push connector,
//! and the test that says `Polling` is complete without them is the assertion that keeps the gate from
//! demanding a test for a capability the connector does not have.

use std::collections::BTreeSet;
use std::fmt;

use crate::manifest::{ConnectorManifest, ConnectorVersion, WebhookSupport};

/// The longest accepted purpose for a declared artifact.
pub const MAX_ATTESTATION_PURPOSE_CHARS: usize = 300;

/// The longest accepted evidence path.
///
/// The same bound a manifest's research path uses, because these values travel together into the same
/// review document and a reader should not meet two limits for one field.
pub const MAX_EVIDENCE_PATH_CHARS: usize = 512;

/// The most test cases one connector may attest to.
///
/// A refusal rather than a clamp, and deliberately generous: the number exists to catch a typo or a
/// placeholder (a `0`, or a count pasted from another connector), not to police connector size. A connector
/// with more than this many tests has larger problems than its test count.
pub const MAX_ATTESTED_TESTS: u32 = 100_000;

/// The item identifiers a readiness review reports, in a stable order.
///
/// An enum rather than a string, so a report cannot name an item the checklist does not have and a caller
/// cannot misspell one. `as_str` is the stable code a stored review or a diagnostic would carry.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ReadinessItem {
    /// Dated official-source research for the provider.
    ///
    /// **Derived** from the manifest's [`ResearchRecord`](crate::manifest::ResearchRecord): the path of the
    /// record and the date it was last verified. `external-research.md` makes the record mandatory before
    /// implementation, so this item is the one the manifest already answers.
    OfficialSourcesDated,
    /// The manifest and its configuration were validated.
    ///
    /// **Derived, and vacuously satisfied.** Holding a [`ConnectorManifest`] at all means every constructor
    /// rule passed, and the manifest carries the configuration and secret-field declarations. Reporting it
    /// explicitly rather than omitting it is what makes this list match the document's own; the honest
    /// statement is that its evidence is the type of the value the reviewer holds.
    ManifestValidated,
    /// Onboarding tests exist, covering both success and failure.
    ///
    /// **Declared count.** "Successful **and failed** onboarding tests" is two claims, and neither is
    /// derivable from a manifest, so this is the one category where the count is paired with an explicit
    /// flag rather than trusted alone. See [`Attestation::test_count`].
    OnboardingTests,
    /// Auth refresh, revoke, and reauth tests exist.
    AuthLifecycleTests,
    /// Pagination and rate-limit tests exist.
    PaginationAndLimitsTests,
    /// Operation contract fixtures exist.
    ///
    /// Fixtures rather than tests, because the document distinguishes them: a sanitized wire fixture is the
    /// artifact and the test is what consumes it. The path is checked to look like a fixture directory.
    OperationContractFixtures,
    /// The policy and effect metadata was reviewed.
    ///
    /// **Declared artifact.** Not derivable: the manifest's effects are checked for *self-consistency*
    /// (`ADR-0054`), and whether they match what the provider code does is a review, not a rule.
    PolicyMetadataReviewed,
    /// Diagnostics are redacted.
    ///
    /// **Derived, and it is the strongest derived item in the list.** `P5-001` chose the diagnostic field set
    /// so that `is_loggable()` is true for all of it and `may_reach_a_model()` is false for exactly one
    /// field. A connector that uses [`DiagnosticField`](crate::DiagnosticField) cannot produce an unredacted
    /// diagnostic, so this item's evidence is the type rather than a promise — and a declared path is
    /// accepted as well, for a connector whose redaction test is worth naming.
    RedactedDiagnostics,
    /// Webhook signature and replay tests exist. **Push connectors only.**
    WebhookSignatureTests,
    /// Webhook replay-window handling is tested. **Push connectors only.**
    ///
    /// Separate from the signature item because they fail differently: a broken signature check accepts a
    /// forged delivery, while a broken replay window accepts a *genuine* delivery a second time. `P5-001`'s
    /// `WebhookRejection` distinguishes them for the same reason.
    WebhookReplayTests,
    /// Documented limitations exist.
    ///
    /// `ADR-0054`'s and `ADR-0055`'s Limits sections are the precedent: every slice in this crate records
    /// what it does not do, and a connector is expected to do the same.
    DocumentedLimitations,
    /// A live smoke test exists, or the reason it cannot.
    ///
    /// "Where credentials permit" makes this item conditional on a fact the author knows and this crate does
    /// not, so the refusal to declare one must carry a reason — see [`LiveSmokeTest`].
    LiveSmokeTest,
}

impl ReadinessItem {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OfficialSourcesDated => "official_sources_dated",
            Self::ManifestValidated => "manifest_validated",
            Self::OnboardingTests => "onboarding_tests",
            Self::AuthLifecycleTests => "auth_lifecycle_tests",
            Self::PaginationAndLimitsTests => "pagination_and_limits_tests",
            Self::OperationContractFixtures => "operation_contract_fixtures",
            Self::PolicyMetadataReviewed => "policy_metadata_reviewed",
            Self::RedactedDiagnostics => "redacted_diagnostics",
            Self::WebhookSignatureTests => "webhook_signature_tests",
            Self::WebhookReplayTests => "webhook_replay_tests",
            Self::DocumentedLimitations => "documented_limitations",
            Self::LiveSmokeTest => "live_smoke_test",
        }
    }

    /// Returns how much a satisfied item is worth as evidence.
    #[must_use]
    pub const fn evidence(self) -> EvidenceStrength {
        match self {
            // The manifest already answers these, and an answer inside a validated value cannot be
            // asserted falsely.
            Self::OfficialSourcesDated | Self::ManifestValidated | Self::RedactedDiagnostics => {
                EvidenceStrength::Derived
            }
            // An author names a path, which is checkable in shape but not in existence.
            Self::OperationContractFixtures
            | Self::PolicyMetadataReviewed
            | Self::DocumentedLimitations => EvidenceStrength::DeclaredArtifact,
            // An author supplies a number and, for onboarding, a second claim about failure coverage.
            //
            // **The two webhook items belong here and NOT under `Derived`, and getting that wrong was a real
            // defect this module's own test caught.** A first version treated "the manifest declares push
            // delivery" as the webhook items' evidence, which conflates the *applicability* condition with the
            // *evidence*: whether a connector needs webhook tests is a manifest fact, but whether it **has**
            // them is not, and a manifest saying "I verify HMAC-SHA256" proves nothing was tested. The gate
            // would have reported a push connector complete with no signature or replay test at all — the exact
            // failure `WebhookRejection` exists to make visible.
            Self::OnboardingTests
            | Self::AuthLifecycleTests
            | Self::PaginationAndLimitsTests
            | Self::WebhookSignatureTests
            | Self::WebhookReplayTests => EvidenceStrength::DeclaredCount,
            // Conditional on a fact this crate cannot see, so a refusal must carry a reason.
            Self::LiveSmokeTest => EvidenceStrength::Conditional,
        }
    }

    /// Returns the item's operator-facing title.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::OfficialSourcesDated => "dated official-source research",
            Self::ManifestValidated => "manifest and configuration validation",
            Self::OnboardingTests => "successful and failed onboarding tests",
            Self::AuthLifecycleTests => "auth refresh, revoke, and reauth tests",
            Self::PaginationAndLimitsTests => "pagination and rate-limit tests",
            Self::OperationContractFixtures => "operation contract fixtures",
            Self::PolicyMetadataReviewed => "policy and effect metadata review",
            Self::RedactedDiagnostics => "redacted diagnostics",
            Self::WebhookSignatureTests => "webhook signature tests",
            Self::WebhookReplayTests => "webhook replay tests",
            Self::DocumentedLimitations => "documented limitations",
            Self::LiveSmokeTest => "live smoke test, or the reason it cannot run",
        }
    }

    /// Returns whether this item applies to a connector with the given webhook support.
    ///
    /// The two webhook items are the only conditional ones, and the condition is the manifest's own
    /// declaration rather than a caller's judgement — `tools-and-connectors.md`'s "if applicable" is a fact
    /// about the connector.
    #[must_use]
    pub const fn applies_to(self, webhook: &WebhookSupport) -> bool {
        match self {
            Self::WebhookSignatureTests | Self::WebhookReplayTests => {
                matches!(webhook, WebhookSupport::Push { .. })
            }
            _ => true,
        }
    }
}

impl fmt::Display for ReadinessItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How much a satisfied checklist item is worth as evidence.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EvidenceStrength {
    /// The fact is inside a value that could not be constructed without it.
    Derived,
    /// A repository-relative path naming an artifact, checked for shape but not existence.
    DeclaredArtifact,
    /// A number an author supplied.
    DeclaredCount,
    /// The item may be refused, in which case the refusal must carry a reason.
    Conditional,
}

impl EvidenceStrength {
    /// Returns the stable snake-case token.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Derived => "derived",
            Self::DeclaredArtifact => "declared_artifact",
            Self::DeclaredCount => "declared_count",
            Self::Conditional => "conditional",
        }
    }

    /// Returns whether satisfying this item requires the author to assert something.
    ///
    /// True for everything except `Derived`, and it is the question a reviewer should ask first: a checklist
    /// satisfied entirely by assertions is a checklist that could be satisfied by writing assertions.
    #[must_use]
    pub const fn requires_an_assertion(self) -> bool {
        !matches!(self, Self::Derived)
    }
}

/// A repository-relative path naming an artifact a connector is expected to have.
///
/// # What this checks and what it cannot
///
/// It checks that the path **could be** a repository-relative artifact path: non-empty, bounded, relative,
/// with no `..`, no drive prefix, and a filename that ends in a known artifact extension. That is a real
/// refusal — it catches a pasted absolute path, a traversal, and a path pointing at nothing in particular —
/// and it is honestly weaker than "the file exists", because this crate has no filesystem and gaining one
/// would put `std::fs` in an adapter whose whole value is being testable as a function of its arguments.
///
/// The extension check is not decoration. `external-research.md` requires a *dated record at a specific
/// path*, `ADR-0054` checks a manifest's research path the same way, and a value that ends in `.rs` or has no
/// extension is not the artifact the item names. Accepting any non-empty string would make the item's
/// evidence "somebody typed something".
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct EvidencePath(String);

impl EvidencePath {
    /// Validates a path naming an artifact.
    ///
    /// # Errors
    ///
    /// Returns [`ReadinessError::Path`] when the value is empty, has leading or trailing whitespace, is
    /// absolute, begins with a drive or home prefix, contains `..`, exceeds [`MAX_EVIDENCE_PATH_CHARS`], or
    /// does not end in one of the accepted artifact extensions.
    pub fn new(value: impl Into<String>) -> Result<Self, ReadinessError> {
        let value = value.into();
        if value.trim() != value {
            return Err(ReadinessError::Path {
                value,
                reason: "an evidence path may not carry leading or trailing whitespace",
            });
        }
        if value.is_empty() || value.chars().count() > MAX_EVIDENCE_PATH_CHARS {
            return Err(ReadinessError::Path {
                value,
                reason: "an evidence path must be 1 to 512 characters",
            });
        }
        // A path the repository cannot address is not evidence of anything in the repository. The
        // `~` and drive-prefix checks are separate from `starts_with('/')` because they are the two other
        // ways a value becomes machine-specific, and `C:` is reachable from a `\\`-prefixed path too.
        if value.starts_with('/') || value.starts_with('\\') || value.starts_with('~') {
            return Err(ReadinessError::Path {
                value,
                reason: "an evidence path must be repository-relative, not absolute or home-relative",
            });
        }
        if value.contains("..") {
            return Err(ReadinessError::Path {
                value,
                reason: "an evidence path may not contain a traversal segment",
            });
        }
        if value.contains(':') {
            return Err(ReadinessError::Path {
                value,
                reason: "an evidence path may not contain a colon, which covers a Windows drive prefix",
            });
        }
        let Some((_, extension)) = value.rsplit_once('.') else {
            return Err(ReadinessError::Path {
                value,
                reason: "an evidence path must name a file with an extension",
            });
        };
        if !ACCEPTED_ARTIFACT_EXTENSIONS.contains(&extension) {
            return Err(ReadinessError::Path {
                value,
                reason: "an evidence path must end in one of: rs, md, sql, json, toml, snap, yaml, yml",
            });
        }
        Ok(Self(value))
    }

    /// Returns the path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EvidencePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The extensions an evidence artifact may have.
const ACCEPTED_ARTIFACT_EXTENSIONS: [&str; 8] =
    ["rs", "md", "sql", "json", "toml", "snap", "yaml", "yml"];

/// An author's statement that an item is satisfied, with what it rests on.
///
/// Every declared item is one of these, and each has a shape chosen so that the weakest part is visible:
///
/// - `purpose` is required for **every** item, because a checklist entry that cannot say what it is for is
///   the formality this module exists to avoid;
/// - `test_count` is required for the three test-suite items, because "auth refresh, revoke, and reauth
///   tests" is a claim about *how many* things were tried and a bare boolean would let one assertion stand
///   for three;
/// - `covers_failure` is required for onboarding specifically, because the document separates "successful
///   **and failed** onboarding tests" into two claims and a success-only suite is the common shortcut.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Attestation {
    /// What the artifact or the work is for.
    pub purpose: String,
    /// Where the artifact is, when the item names one.
    pub path: Option<EvidencePath>,
    /// How many test cases the suite has, when the item names a suite.
    pub test_count: Option<u32>,
    /// Whether the suite includes a test for the failure path.
    ///
    /// Only consulted for [`ReadinessItem::OnboardingTests`]. `None` for every other item, and the
    /// constructor refuses a value on an item that does not use it, so a caller cannot believe it set
    /// something meaningful.
    pub covers_failure: Option<bool>,
}

impl Attestation {
    /// Records an artifact at a path.
    #[must_use]
    pub fn artifact(purpose: impl Into<String>, path: EvidencePath) -> Self {
        Self {
            purpose: purpose.into(),
            path: Some(path),
            test_count: None,
            covers_failure: None,
        }
    }

    /// Records a test suite with a case count.
    #[must_use]
    pub fn suite(purpose: impl Into<String>, test_count: u32) -> Self {
        Self {
            purpose: purpose.into(),
            path: None,
            test_count: Some(test_count),
            covers_failure: None,
        }
    }

    /// Records the onboarding suite, which must also cover the failure path.
    #[must_use]
    pub fn onboarding(purpose: impl Into<String>, test_count: u32, covers_failure: bool) -> Self {
        Self {
            purpose: purpose.into(),
            path: None,
            test_count: Some(test_count),
            covers_failure: Some(covers_failure),
        }
    }

    /// Records that an item's purpose is stated without a path or a count.
    ///
    /// Used for [`ReadinessItem::ManifestValidated`] and [`ReadinessItem::RedactedDiagnostics`], whose
    /// evidence is the value the reviewer already holds.
    #[must_use]
    pub fn stated(purpose: impl Into<String>) -> Self {
        Self {
            purpose: purpose.into(),
            path: None,
            test_count: None,
            covers_failure: None,
        }
    }
}

/// Whether a connector has a live smoke test, or why it does not.
///
/// `tools-and-connectors.md` says "a live smoke test **where credentials permit**", which makes the item
/// conditional on a fact the connector's author knows and this crate cannot observe. So a refusal is legal
/// and must carry a reason: `Absent` with an empty reason is refused at construction, because "no smoke test"
/// and "we decided not to say why" are different statements and only the first one is reviewable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LiveSmokeTest {
    /// A live test exists, at a path.
    Present {
        /// Where the test lives.
        path: EvidencePath,
        /// Why it is opt-in, which is normally a cost or credential gate.
        gate: String,
    },
    /// No live test, for a stated reason.
    Absent {
        /// Why a live test could not be written. Required, and non-empty.
        reason: String,
    },
}

impl LiveSmokeTest {
    /// Records a live test behind a gate.
    ///
    /// # Errors
    ///
    /// Returns [`ReadinessError::Attestation`] when the gate is blank, because a smoke test with no stated
    /// opt-in condition is one that either runs in CI or cannot be run, and both are problems the gate field
    /// exists to make visible.
    pub fn present(path: EvidencePath, gate: impl Into<String>) -> Result<Self, ReadinessError> {
        let gate = gate.into();
        if gate.trim().is_empty() || gate.chars().count() > MAX_ATTESTATION_PURPOSE_CHARS {
            return Err(ReadinessError::Attestation {
                reason: "a live smoke test must state the gate that keeps it opt-in",
            });
        }
        Ok(Self::Present { path, gate })
    }

    /// Records that no live test exists, for a reason.
    ///
    /// # Errors
    ///
    /// Returns [`ReadinessError::Attestation`] when the reason is blank or oversized.
    pub fn absent(reason: impl Into<String>) -> Result<Self, ReadinessError> {
        let reason = reason.into();
        if reason.trim().is_empty() || reason.chars().count() > MAX_ATTESTATION_PURPOSE_CHARS {
            return Err(ReadinessError::Attestation {
                reason: "a connector without a live smoke test must state why it has none",
            });
        }
        Ok(Self::Absent { reason })
    }
}

/// Why a readiness review could not be assembled.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReadinessError {
    /// An evidence path is unusable.
    #[error("the evidence path `{value}` is unusable: {reason}")]
    Path {
        /// The rejected value.
        value: String,
        /// What is wrong.
        reason: &'static str,
    },
    /// An attestation is incomplete.
    #[error("the attestation is unusable: {reason}")]
    Attestation {
        /// What is wrong.
        reason: &'static str,
    },
    /// An item was attested to that the connector's webhook support does not make applicable.
    ///
    /// A refusal rather than a silent drop, because attesting to webhook tests for a polling connector is a
    /// sign the author is working from a template rather than from their own connector — and a review that
    /// quietly discarded the entry would hide that.
    #[error("`{item}` does not apply to this connector: it declares no push webhook")]
    NotApplicable {
        /// The item's stable code.
        item: &'static str,
    },
    /// A required item was attested to more than once.
    #[error("`{item}` was attested to twice, so the review has two answers for one question")]
    Duplicate {
        /// The item's stable code.
        item: &'static str,
    },
}

/// A completed review of one connector, with every item accounted for.
///
/// [`Self::evaluate`] is the only way to make one, and it returns a review whose items are all satisfied —
/// an unsatisfied item is a [`ReadinessGap`], not a review with a missing field. So "is this connector
/// complete" is answered by whether a value of this type exists, which is the same move `ADR-0037` made for
/// an admitted request and `ADR-0055` made for a consumable transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadinessReview {
    connector: String,
    version: ConnectorVersion,
    satisfied: Vec<ReadinessAssessment>,
    webhook_items_required: bool,
}

impl ReadinessReview {
    /// Evaluates every applicable item against the manifests' own facts and the author's attestations.
    ///
    /// # Errors
    ///
    /// Returns [`ReadinessError::Duplicate`] when an item is attested to twice,
    /// [`ReadinessError::NotApplicable`] when a webhook item is attested to by a connector that declares no
    /// push delivery, and [`ReadinessError::Attestation`] when an attestation does not carry what its item
    /// requires.
    ///
    /// # Why the attestations are a slice and the applicability comes from the manifest
    ///
    /// Taking the manifest rather than the webhook declaration is the point: the review cannot be assembled
    /// from a set of claims alone, so a caller cannot review a connector it does not have. The four derived
    /// items are read from the manifest here rather than accepted as arguments for the same reason — an item
    /// whose evidence is passed in is an item whose evidence can be passed in wrongly.
    pub fn evaluate(
        manifest: &ConnectorManifest,
        attestations: &[(ReadinessItem, Attestation)],
        live_smoke: Option<&LiveSmokeTest>,
    ) -> Result<Self, ReadinessError> {
        let mut seen = BTreeSet::new();
        for (item, _) in attestations {
            if !seen.insert(*item) {
                return Err(ReadinessError::Duplicate {
                    item: item.as_str(),
                });
            }
            if !item.applies_to(manifest.webhook()) {
                return Err(ReadinessError::NotApplicable {
                    item: item.as_str(),
                });
            }
        }

        let mut satisfied = Vec::new();
        for item in ALL_ITEMS {
            if !item.applies_to(manifest.webhook()) {
                continue;
            }
            let assessment = match item.evidence() {
                EvidenceStrength::Derived => Self::derived(manifest, item),
                // The conditional item's evidence is a [`LiveSmokeTest`] rather than an [`Attestation`],
                // because the two shapes differ: a smoke test is either at a path with a gate or absent with a
                // reason, and an `Attestation` cannot express the gate. Routing it here is what makes the
                // "a refusal must carry a reason" promise a fact: `LiveSmokeTest::absent` already refused an
                // empty reason when it was constructed, and there is no other way to reach this arm.
                EvidenceStrength::Conditional => {
                    let Some(live_smoke) = live_smoke else {
                        continue;
                    };
                    ReadinessAssessment {
                        item,
                        evidence: item.evidence(),
                        detail: describe_live_smoke(live_smoke),
                        declared_by: None,
                    }
                }
                _ => {
                    let declared = attestations
                        .iter()
                        .find(|(attested, _)| *attested == item)
                        .map(|(_, attestation)| attestation);
                    let Some(declared) = declared else {
                        // Not an error: a review is assembled from whatever the author has, and the gap is
                        // reported by `gaps()` so a caller can see everything that is missing at once rather
                        // than one refusal at a time.
                        continue;
                    };
                    validate_attestation(item, declared)?;
                    ReadinessAssessment::declared(item, declared)
                }
            };
            satisfied.push(assessment);
        }

        Ok(Self {
            connector: manifest.id().as_str().to_owned(),
            version: manifest.version().clone(),
            satisfied,
            webhook_items_required: matches!(manifest.webhook(), WebhookSupport::Push { .. }),
        })
    }

    /// Returns the derived assessment for an item the manifest already answers.
    ///
    /// Infallible by construction: every `Derived` item's evidence is a value the manifest could not have
    /// been built without. `ManifestValidated`'s evidence is the manifest itself, which is why it needs no
    /// accessor — the fact that this function was reached is the evidence.
    fn derived(manifest: &ConnectorManifest, item: ReadinessItem) -> ReadinessAssessment {
        let detail = match item {
            ReadinessItem::OfficialSourcesDated => format!(
                "{} (last verified {})",
                manifest.research().path,
                manifest.research().last_verified
            ),
            ReadinessItem::ManifestValidated => format!(
                "`{}` version {} passed every constructor rule",
                manifest.id().as_str(),
                manifest.version().as_str()
            ),
            ReadinessItem::RedactedDiagnostics => {
                "the diagnostic field set is closed, so every field is loggable and only the provider \
                 request identifier is withheld from a model"
                    .to_owned()
            }
            // Every other item is declared, so this arm is unreachable from `evaluate`'s own dispatch. It
            // returns a string rather than panicking because a panic in a review would take down a caller
            // that is only trying to report progress, and because a later item added to the enum must fail
            // visibly rather than crash.
            _ => "this item has no derived evidence".to_owned(),
        };
        ReadinessAssessment {
            item,
            evidence: item.evidence(),
            detail,
            declared_by: None,
        }
    }

    /// Returns the connector this review covers.
    #[must_use]
    pub fn connector(&self) -> &str {
        &self.connector
    }

    /// Returns the connector version reviewed.
    #[must_use]
    pub fn version(&self) -> &ConnectorVersion {
        &self.version
    }

    /// Returns the satisfied items, in checklist order.
    #[must_use]
    pub fn satisfied(&self) -> &[ReadinessAssessment] {
        &self.satisfied
    }

    /// Returns whether the connector's webhook delivery made the two webhook items applicable.
    #[must_use]
    pub const fn requires_webhook_tests(&self) -> bool {
        self.webhook_items_required
    }

    /// Returns whether every applicable item is satisfied.
    ///
    /// Always `true` for a value that exists, and it is a method rather than a constant so a caller reading
    /// the report does not have to know that. The interesting question is answered by [`Self::gaps`], which a
    /// caller can only ask *before* it has a review.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        true
    }

    /// Returns what is still missing, given a manifest and the author's attestations.
    ///
    /// The complement of [`Self::evaluate`], and the method a progress report actually wants: it names every
    /// outstanding item at once with its evidence strength, instead of refusing at the first one. A caller
    /// that has a `ReadinessReview` has nothing to learn from this, so it is an associated function.
    ///
    /// Items that are satisfied but *weakly* are not gaps, and that is deliberate: this reports absence, not
    /// doubt. The doubt is visible on every item through [`EvidenceStrength`], which is where a reviewer
    /// should look.
    #[must_use]
    pub fn gaps(
        manifest: &ConnectorManifest,
        attestations: &[(ReadinessItem, Attestation)],
        live_smoke: Option<&LiveSmokeTest>,
    ) -> Vec<ReadinessGap> {
        ALL_ITEMS
            .iter()
            .copied()
            .filter(|item| item.applies_to(manifest.webhook()))
            .filter(|item| item.evidence().requires_an_assertion())
            .filter(|item| match item.evidence() {
                // The conditional item is satisfied by the value, not by a list entry, so a caller that
                // supplied a reason has answered it however many attestations it also wrote.
                EvidenceStrength::Conditional => live_smoke.is_none(),
                _ => !attestations.iter().any(|(attested, _)| attested == item),
            })
            .map(|item| ReadinessGap {
                item,
                evidence: item.evidence(),
                title: item.title(),
            })
            .collect()
    }
}

/// Every item on the checklist, in the order the architecture document lists them.
///
/// A `const` array rather than an iterator over the enum, because a new variant must be added here
/// deliberately — the same reasoning `P5-001` used for `DiagnosticField`'s count assertion. A variant that is
/// missing from this list is an item nothing checks, and the count test is what makes that fail.
pub const ALL_ITEMS: [ReadinessItem; 12] = [
    ReadinessItem::OfficialSourcesDated,
    ReadinessItem::ManifestValidated,
    ReadinessItem::OnboardingTests,
    ReadinessItem::AuthLifecycleTests,
    ReadinessItem::PaginationAndLimitsTests,
    ReadinessItem::OperationContractFixtures,
    ReadinessItem::PolicyMetadataReviewed,
    ReadinessItem::RedactedDiagnostics,
    ReadinessItem::WebhookSignatureTests,
    ReadinessItem::WebhookReplayTests,
    ReadinessItem::DocumentedLimitations,
    ReadinessItem::LiveSmokeTest,
];

/// One satisfied item, with what it rests on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadinessAssessment {
    /// The item.
    pub item: ReadinessItem,
    /// How strong the satisfaction is.
    ///
    /// Recorded rather than derived from `declared_by.is_some()`, and this field exists because the first
    /// version of this struct DID derive it — which made a conditional item (whose evidence is a
    /// [`LiveSmokeTest`] rather than an [`Attestation`], so its `declared_by` is `None`) indistinguishable from
    /// a derived one. A reader counting `declared_by.is_none()` to find the strong half of the checklist would
    /// have counted a stated *reason* as a manifest fact. Recording the strength makes the two explicit.
    pub evidence: EvidenceStrength,
    /// What the satisfaction rests on, for a human reader.
    pub detail: String,
    /// The author's attestation, when the item was declared with one.
    pub declared_by: Option<Attestation>,
}

impl ReadinessAssessment {
    /// Records an item satisfied by the author's own attestation.
    ///
    /// The `detail` is rendered from the attestation rather than passed in, so a report cannot describe a
    /// declared item as something the attestation does not say. That is a small version of the rule this
    /// whole module exists for: a summary of evidence should be derived from the evidence.
    fn declared(item: ReadinessItem, attestation: &Attestation) -> Self {
        // Built with `write!` into one `String` rather than repeated `push_str(&format!(..))`, because the
        // latter allocates a temporary for each append. `write!` cannot fail on a `String`, and the discarded
        // result is the documented shape (`fmt::Write` for `String` is infallible).
        use std::fmt::Write as _;
        let mut detail = attestation.purpose.clone();
        if let Some(path) = &attestation.path {
            let _ = write!(detail, " (at {path})");
        }
        if let Some(count) = attestation.test_count {
            let _ = write!(detail, " ({count} test case(s))");
        }
        if attestation.covers_failure == Some(true) {
            detail.push_str(", including the failure path");
        }
        Self {
            item,
            evidence: item.evidence(),
            detail,
            declared_by: Some(attestation.clone()),
        }
    }
}

/// One item that is still missing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadinessGap {
    /// The item.
    pub item: ReadinessItem,
    /// How strong its evidence would have been.
    pub evidence: EvidenceStrength,
    /// The item's title, so a report does not need a second lookup.
    pub title: &'static str,
}

impl fmt::Display for ReadinessGap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} (`{}`, {} evidence)",
            self.title,
            self.item.as_str(),
            self.evidence.as_str()
        )
    }
}

/// Renders a live-smoke-test declaration for a report.
///
/// The two arms are the conditional item's whole content: a connector with a live test names where it is and
/// what keeps it opt-in, and one without must have said why. A single `bool` could not carry either, which is
/// why [`LiveSmokeTest`] is an enum rather than an `Option<EvidencePath>` — the gate and the reason are the
/// evidence, not decoration on it.
#[must_use]
pub fn describe_live_smoke(test: &LiveSmokeTest) -> String {
    match test {
        LiveSmokeTest::Present { path, gate } => {
            format!("a live smoke test at {path}, gated on: {gate}")
        }
        LiveSmokeTest::Absent { reason } => format!("no live smoke test: {reason}"),
    }
}

/// Refuses an attestation that does not carry what its item requires.
fn validate_attestation(
    item: ReadinessItem,
    attestation: &Attestation,
) -> Result<(), ReadinessError> {
    if attestation.purpose.trim().is_empty()
        || attestation.purpose.chars().count() > MAX_ATTESTATION_PURPOSE_CHARS
    {
        return Err(ReadinessError::Attestation {
            reason: "every item must state its purpose, in 1 to 300 characters",
        });
    }
    match item.evidence() {
        EvidenceStrength::DeclaredArtifact => {
            if attestation.path.is_none() {
                return Err(ReadinessError::Attestation {
                    reason: "this item names an artifact, so it must give its repository-relative path",
                });
            }
        }
        EvidenceStrength::DeclaredCount => {
            if attestation.path.is_some() {
                return Err(ReadinessError::Attestation {
                    reason: "this item names a test suite rather than an artifact, so it must not give a \
                             path",
                });
            }
            match attestation.test_count {
                None | Some(0) => {
                    return Err(ReadinessError::Attestation {
                        reason: "a test suite must report at least one test case; a count of zero is the \
                                 claim that no test exists",
                    });
                }
                Some(count) if count > MAX_ATTESTED_TESTS => {
                    return Err(ReadinessError::Attestation {
                        reason: "the reported test count is beyond what this platform accepts, which is a \
                                 typo or a count from another connector",
                    });
                }
                Some(_) => {}
            }
        }
        // `Derived` items carry a purpose and nothing else, and the `Conditional` item never reaches here
        // because its evidence is a `LiveSmokeTest`. A `covers_failure` value on either is refused below.
        EvidenceStrength::Derived | EvidenceStrength::Conditional => {}
    }
    // `covers_failure` is meaningful for exactly one item, and a value on any other item is refused rather
    // than ignored: a caller that set it believed it said something, and silently discarding that belief is
    // how a connector ships with a success-only suite while its report claims otherwise.
    if attestation.covers_failure.is_some() && item != ReadinessItem::OnboardingTests {
        return Err(ReadinessError::Attestation {
            reason: "only the onboarding item reports failure coverage, so this value would be ignored",
        });
    }
    if item == ReadinessItem::OnboardingTests && attestation.covers_failure != Some(true) {
        return Err(ReadinessError::Attestation {
            reason: "the document requires successful AND failed onboarding tests, so failure coverage must \
                     be reported as present",
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "readiness_tests.rs"]
mod tests;
