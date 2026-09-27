//! Google's OAuth scope categories, and the verification burden each one carries.
//!
//! # Why a category is not just a label
//!
//! Google classifies every scope as **non-sensitive**, **sensitive**, or **restricted**, and the category —
//! not the scope string — decides what a deployment must do before it may ship. From the Gmail scopes page
//! read on 2026-09-27:
//!
//! > Non-sensitive: These scopes provide the smallest scope of authorization and only require basic OAuth App
//! > Verification.
//! >
//! > Sensitive: … They require additional OAuth App Verification.
//! >
//! > Restricted: These scopes provide wide access to Google user data and require restricted scope OAuth App
//! > Verification.
//! >
//! > **If you store restricted scope data on servers (or transmit), then you must go through a security
//! > assessment.**
//!
//! So the category is a **review burden**, and it is the one fact that decides whether deploying this
//! connector needs a Google security assessment. The research record's own Verification Plan asks for a test
//! "that a Gmail connector's declared scopes are all *accounted for* against a table of Google's categories,
//! so adding a scope forces a decision about the verification burden", and this module is the type that makes
//! such an accounting meaningful rather than a list of strings a reader has to interpret.
//!
//! # The one property worth reading the code for
//!
//! [`ScopeCategory::Unknown`] means **the published table does not list this scope**, and it must not be read
//! as "not sensitive". A scope a deployment has not accounted for is one whose burden is **not established**,
//! and [`ScopeAccounting::burden`] therefore reports the whole deployment as unestablished rather than
//! summing the scopes it does know about. Under-reporting a burden is the failure that matters here: an
//! operator who is told "basic review" and then ships a restricted scope has skipped an assessment Google
//! requires, and nothing in the deployment would have said so.
//!
//! The same reasoning as `RateLimitEvidence`: a value that is *assumed* and a value that is *documented* are
//! different facts, and collapsing them is how an assumption becomes a claim.
//!
//! # What this module does not claim
//!
//! **Nothing here is a statement about a deployment's actual compliance.** The category is Google's own
//! classification of a scope string; whether a particular JARVIS deployment needs the assessment also depends
//! on facts this crate cannot see — above all the **internal-app exemption**, where an app "used only inside
//! one Google Workspace organization" does not require further review for restricted or sensitive scopes.
//! That is a property of the consent screen's audience setting, not of the scope set, so it is recorded in the
//! research record and deliberately **not** modelled here as a field that could be set wrongly.

use std::fmt;

/// The date the category table was read from Google's published pages.
///
/// The same date the research record's own `last_verified` carries, and asserted equal to it — a table dated
/// differently from the record it came from would let a recategorisation look like it had been accounted for.
pub const SCOPE_CATEGORIES_RECORDED_ON: &str = "2026-09-27";

/// Google's classification of one scope string.
///
/// A closed set, because the page publishes exactly three categories and a fourth situation this crate
/// observes: a scope the page does not list.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ScopeCategory {
    /// The smallest authorization; only basic app verification.
    NonSensitive,
    /// Access to specific user data; additional app verification.
    Sensitive,
    /// Wide access to user data; restricted-scope verification, and a security assessment when the data is
    /// stored or transmitted through a server.
    Restricted,
    /// Google's published table **does not list this scope**.
    ///
    /// Not a synonym for [`Self::NonSensitive`], and the distinction is the whole point of this module. It
    /// arises for two different reasons that a caller cannot tell apart from here, and both are real in this
    /// connector: a scope belonging to **another** Google API whose page was not read (Calendar), and a scope
    /// defined by a **different specification entirely** — the `openid` scope, which belongs to the OIDC
    /// identity standard rather than to a Google API.
    Unknown,
}

impl ScopeCategory {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NonSensitive => "non_sensitive",
            Self::Sensitive => "sensitive",
            Self::Restricted => "restricted",
            Self::Unknown => "unknown",
        }
    }

    /// Returns whether Google's table establishes a category for this scope.
    ///
    /// The predicate a caller asks before treating a category as an answer, so "we know this is cheap" and
    /// "we have no entry for this" cannot be confused at a call site.
    #[must_use]
    pub const fn is_established(self) -> bool {
        !matches!(self, Self::Unknown)
    }

    /// Returns the review burden this category carries.
    #[must_use]
    pub const fn burden(self) -> VerificationBurden {
        match self {
            Self::NonSensitive => VerificationBurden::BasicReview,
            Self::Sensitive => VerificationBurden::AdditionalReview,
            Self::Restricted => VerificationBurden::AdditionalReviewAndConditionalAssessment,
            // **Fail closed.** An unlisted scope is not a cheap scope; it is one whose burden nobody has
            // established. See the module doc for why under-reporting is the direction that harms.
            Self::Unknown => VerificationBurden::Unestablished,
        }
    }

    /// Returns whether a security assessment is required for this category.
    ///
    /// Restricted scopes require one **when the data is stored or transmitted through a server** — the
    /// condition is Google's, not a hedge — so the answer is [`AssessmentRequirement`] rather than a `bool`,
    /// and [`AssessmentRequirement::NotEstablished`] is deliberately a different value from
    /// [`AssessmentRequirement::NotRequired`].
    #[must_use]
    pub const fn assessment(self) -> AssessmentRequirement {
        match self {
            Self::NonSensitive | Self::Sensitive => AssessmentRequirement::NotRequired,
            Self::Restricted => AssessmentRequirement::IfStoredOrTransmitted,
            Self::Unknown => AssessmentRequirement::NotEstablished,
        }
    }
}

impl fmt::Display for ScopeCategory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What a deployment must complete because of the categories its scopes fall into.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum VerificationBurden {
    /// Basic OAuth app verification.
    BasicReview,
    /// Additional OAuth app verification.
    AdditionalReview,
    /// Additional verification, **and** a security assessment when restricted-scope data is stored or
    /// transmitted through a server.
    AdditionalReviewAndConditionalAssessment,
    /// The scopes include at least one whose category Google's table does not establish, so no burden can be
    /// stated for the deployment.
    Unestablished,
}

impl VerificationBurden {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BasicReview => "basic_review",
            Self::AdditionalReview => "additional_review",
            Self::AdditionalReviewAndConditionalAssessment => {
                "additional_review_and_conditional_assessment"
            }
            Self::Unestablished => "unestablished",
        }
    }

    /// Returns whether the deployment may ship with only Google's basic review.
    ///
    /// `false` for [`Self::Unestablished`] — the fail-closed direction. A deployment holding a scope nobody
    /// has categorised has not been shown to need only the basic review, and "we did not check" must not read
    /// as "nothing to do".
    #[must_use]
    pub const fn permits_basic_review_only(self) -> bool {
        matches!(self, Self::BasicReview)
    }
}

impl fmt::Display for VerificationBurden {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Whether a security assessment is required, where the answer depends on how the deployment handles data.
///
/// Three values rather than a `bool`, because the restricted case is **conditional** in Google's own wording
/// and the unknown case has no answer at all. A boolean would make "not required" and "not established" the
/// same value, which is exactly the conflation that lets an unassessed deployment look compliant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssessmentRequirement {
    /// No security assessment is required by this category.
    NotRequired,
    /// A security assessment is required **if** restricted-scope data is stored on a server or transmitted
    /// through one.
    IfStoredOrTransmitted,
    /// The category is not established, so the requirement is not either.
    NotEstablished,
}

impl AssessmentRequirement {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::IfStoredOrTransmitted => "if_stored_or_transmitted",
            Self::NotEstablished => "not_established",
        }
    }

    /// Returns whether an assessment **may** be required, failing closed on an unestablished category.
    ///
    /// `true` for [`Self::IfStoredOrTransmitted`], because whether it applies depends on the deployment's own
    /// data handling and a caller that has not established that must not plan as though it did. `true` also for
    /// [`Self::NotEstablished`], for the same reason one step earlier — nobody has recorded the category, so
    /// nothing rules the assessment out. Only [`Self::NotRequired`] answers `false`, which is the single case
    /// where Google's published rule positively excludes an assessment.
    ///
    /// # Why not an `is_required_regardless` predicate
    ///
    /// The first version of this type had one, and it was **false for every value**: Google's rule is
    /// conditional for the only category that requires an assessment, so no category is ever required
    /// *regardless*. A predicate no value satisfies is a predicate a caller cannot branch on, which is the same
    /// defect as a variant nothing constructs. The question a caller actually has is "may I plan as though no
    /// assessment is needed", and that is what this answers.
    #[must_use]
    pub const fn may_require_an_assessment(self) -> bool {
        match self {
            Self::NotRequired => false,
            Self::IfStoredOrTransmitted | Self::NotEstablished => true,
        }
    }
}

/// One declared scope and the category found for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopedCategory {
    /// The scope string, in the provider's own spelling.
    pub scope: String,
    /// Google's category, or [`ScopeCategory::Unknown`] when the table has no entry.
    pub category: ScopeCategory,
}

/// The result of accounting a connector's declared scopes against the recorded category table.
///
/// Built by [`account`], which is a pure function of the declared scopes and the table so every branch is
/// testable without loading a fixture — the same reasoning
/// `diagnostics_for` records for taking values rather than a connector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScopeAccounting {
    entries: Vec<ScopedCategory>,
}

impl ScopeAccounting {
    /// Returns every declared scope with its category, in declaration order.
    #[must_use]
    pub fn entries(&self) -> &[ScopedCategory] {
        &self.entries
    }

    /// Returns the scopes whose category the table does not establish.
    ///
    /// The list an author must act on: each one is a scope nobody has decided the burden of yet.
    #[must_use]
    pub fn unaccounted(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|entry| !entry.category.is_established())
            .map(|entry| entry.scope.as_str())
            .collect()
    }

    /// Returns whether every declared scope has an established category.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.entries
            .iter()
            .all(|entry| entry.category.is_established())
    }

    /// Returns the burden the deployment carries, **failing closed on any unaccounted scope**.
    ///
    /// The heaviest established burden when every scope is accounted for, and
    /// [`VerificationBurden::Unestablished`] otherwise — because a deployment's burden cannot be stated while
    /// one of its scopes has no established burden, and reporting the known scopes' maximum would present a
    /// partial answer as a complete one.
    #[must_use]
    pub fn burden(&self) -> VerificationBurden {
        if !self.is_complete() {
            return VerificationBurden::Unestablished;
        }
        self.entries
            .iter()
            .map(|entry| entry.category.burden())
            .max()
            // `max` is `None` only for an empty slice, which is a connector declaring no scopes: that is
            // vacuously the basic review rather than an unestablished burden.
            .unwrap_or(VerificationBurden::BasicReview)
    }
}

/// Accounts the declared scopes against a category table.
///
/// `table` pairs each known scope string with its category; a declared scope absent from the table becomes
/// [`ScopeCategory::Unknown`]. Matching is **exact**, because a scope string is an identifier that goes into
/// an authorization request — a case-insensitive or prefix match here would let `gmail.readonly.x` inherit a
/// category from `gmail.readonly`, which is the wrong answer in the direction that under-reports a burden.
#[must_use]
pub fn account(declared: &[String], table: &[(&str, ScopeCategory)]) -> ScopeAccounting {
    let entries = declared
        .iter()
        .map(|scope| ScopedCategory {
            scope: scope.clone(),
            category: table
                .iter()
                .find(|(known, _)| known == scope)
                .map_or(ScopeCategory::Unknown, |(_, category)| *category),
        })
        .collect();
    ScopeAccounting { entries }
}

#[cfg(test)]
#[path = "scopes_tests.rs"]
mod tests;
