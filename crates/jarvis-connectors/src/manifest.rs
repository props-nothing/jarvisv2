//! The connector manifest: what must be true before an operator installs a connector.
//!
//! `docs/architecture/tools-and-connectors.md` requires the manifest to be **"parseable without loading
//! provider code"** and to include a specific list of things. This module is that list, and the ordering
//! below follows the document's own:
//!
//! > stable ID, version, display name, and provider; supported operations and effect metadata; auth methods
//! > and required/optional scopes; webhook and polling capabilities; configuration JSON Schema and secret
//! > fields; data classifications and residency notes; rate-limit and quota documentation links; official
//! > docs, `llms.txt`, OpenAPI/SDK, changelog, and terms links; minimum JARVIS version and compatibility
//! > status.
//!
//! # The manifest is a security document, not a catalogue entry
//!
//! "Parseable without loading provider code" is a **trust** requirement rather than a performance one. The
//! manifest is the only artifact an operator reads before granting a connector access to their mail or
//! calendar, so it is also the only place that can warn them — and it has to do so completely, because the
//! provider code it describes is exactly what has not run yet.
//!
//! Three things follow, and each is a rule this module enforces rather than documents:
//!
//! 1. **Effects are declared per operation, in the manifest.** Not inferred from an operation's shape and
//!    not deferred to the adapter. `P3-008b` established why an adapter's self-report cannot be the input:
//!    risk is a **floor raised by effects**, so the ladder runs opposite to a vendor's incentive. The same
//!    asymmetry applies here, one layer up — a connector that under-reports its effects gets auto-approved
//!    work, and one that over-reports is merely inconvenienced.
//! 2. **A secret is a *field name*, never a value.** [`SecretField`] names what a deployment must supply and
//!    says what kind of value it is; there is no `String` field anywhere in this module that holds token
//!    material. That is the mechanical form of `security.md`'s rule.
//! 3. **The research links are required, not optional.** `docs/development/external-research.md` makes a
//!    dated record mandatory before connector implementation, and a manifest that cannot name where its
//!    vendor's documentation lives is a connector nobody can re-verify after an API change. [`LinkKind`]
//!    names the **specific** documents the research process already requires, so a manifest with only a
//!    marketing homepage link is incomplete rather than merely terse.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use jarvis_tools::EffectSet;
// Re-exported so a manifest author and a reader share one vocabulary rather than a copy of it. The effect
// and scope types belong to `jarvis-tools` (`P3-001`), and restating them here would be the
// "two values that must agree, with nothing holding both" defect one layer out.
pub use jarvis_tools::ToolEffect;

use crate::auth::AuthMethod;
use crate::ratelimit::RateLimit;

/// The longest connector identifier.
///
/// The same bound `ToolId`'s name segment uses, because a connector's identifier becomes a tool namespace
/// (`connector-namespace`), so a longer one could not become a tool at all.
pub const MAX_CONNECTOR_ID_CHARS: usize = 40;

/// The longest operation identifier.
pub const MAX_CONNECTOR_OPERATION_ID_CHARS: usize = 64;

/// The most operations one connector may declare.
///
/// A bound exists because a manifest is parsed before provider code loads and its operation list is
/// proportional to the connector's attack surface: every operation is a tool that policy must classify. This
/// is the registry's own reasoning (`MAX_REGISTERED_TOOLS`) applied one layer earlier, so an oversized
/// connector is refused at manifest parse rather than at registration.
pub const MAX_CONNECTOR_OPERATIONS: usize = 64;

/// The most scopes one connector may declare.
pub const MAX_CONNECTOR_SCOPES: usize = 64;

/// The most secret fields one connector may declare.
pub const MAX_CONNECTOR_SECRET_FIELDS: usize = 16;

/// The most documentation links one connector may declare.
pub const MAX_MANIFEST_LINKS: usize = 16;

/// The oldest JARVIS version a manifest may claim compatibility with.
///
/// `P5-001` adds the first connector contract, so no released version precedes it. A manifest naming an
/// older minimum would be claiming compatibility with a platform that had no connector surface at all —
/// which reads as "this works on your older install" and cannot be true.
pub const MIN_SUPPORTED_JARVIS_VERSION: &str = "0.5.0";

/// A connector's stable identifier.
///
/// Lowercase ASCII, digits, and `-`/`_`, bounded — the same alphabet [`jarvis_tools::ToolId`]'s namespace
/// segments permit, because this value becomes one. Refusing an unusable identifier here means a connector
/// cannot be **named** in a way it can never be **called**, which is the defect `jarvis-mcp`'s
/// `ServableName` had to work around for MCP tool names.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct ConnectorId(String);

impl ConnectorId {
    /// Validates and records a connector identifier.
    ///
    /// # Errors
    ///
    /// Returns [`ConnectorError::Identifier`] when the value is empty, oversized, starts or ends with a
    /// separator, contains a doubled separator, or holds a character outside the alphabet.
    pub fn new(value: impl Into<String>) -> Result<Self, ConnectorError> {
        let value = value.into();
        if value.is_empty() || value.chars().count() > MAX_CONNECTOR_ID_CHARS {
            return Err(ConnectorError::Identifier {
                value,
                reason: "a connector identifier must be 1 to 40 characters",
            });
        }
        let mut previous_separator = true; // A leading separator is refused by this initial value.
        for character in value.chars() {
            let separator = character == '-' || character == '_';
            if !(character.is_ascii_lowercase() || character.is_ascii_digit() || separator) {
                return Err(ConnectorError::Identifier {
                    value,
                    reason: "a connector identifier may hold only lowercase ASCII letters, digits, `-`, \
                             and `_`",
                });
            }
            if separator && previous_separator {
                return Err(ConnectorError::Identifier {
                    value,
                    reason: "a connector identifier may not start with, end with, or double a separator",
                });
            }
            previous_separator = separator;
        }
        // `previous_separator` is true here only if the value ended with a separator.
        if previous_separator {
            return Err(ConnectorError::Identifier {
                value,
                reason: "a connector identifier may not start with, end with, or double a separator",
            });
        }
        Ok(Self(value))
    }

    /// Returns the identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConnectorId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for ConnectorId {
    type Error = ConnectorError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ConnectorId> for String {
    fn from(value: ConnectorId) -> Self {
        value.0
    }
}

/// A connector manifest version.
///
/// Opaque text, like `jarvis_models::EmbeddingVersion`: it is whatever the connector's author states, and
/// nothing here parses it. A version this crate tried to *order* would be a version it could mis-order,
/// while `docs/architecture/repositories.md`'s rule for tool definitions is that a version is a **digest or
/// a stable identifier**, not a counter to be reasoned about.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct ConnectorVersion(String);

impl ConnectorVersion {
    /// The longest accepted version string.
    pub const MAX_BYTES: usize = 64;

    /// Records a version.
    ///
    /// # Errors
    ///
    /// Returns [`ConnectorError::Version`] when the value is empty, oversized, or holds a control character
    /// that could forge a log line.
    pub fn new(value: impl Into<String>) -> Result<Self, ConnectorError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_BYTES || value.chars().any(char::is_control)
        {
            return Err(ConnectorError::Version { value });
        }
        Ok(Self(value))
    }

    /// Returns the version text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConnectorVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for ConnectorVersion {
    type Error = ConnectorError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<ConnectorVersion> for String {
    fn from(value: ConnectorVersion) -> Self {
        value.0
    }
}

/// What kind of document a [`DocumentationLinks`] entry points at.
///
/// **The variants are the research process's own required sources**, not a generic taxonomy.
/// `docs/development/external-research.md` says to check, in order, "the vendor's repository-local
/// `llms.txt` or `llms-full.txt`, official documentation, official specification, official SDK source, then
/// release notes/changelog". Naming each one is what makes "the manifest has a link" mean something: a
/// homepage is not a specification, and a connector whose only link is its provider's marketing page cannot
/// be re-verified when the API changes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// The vendor's `llms.txt` or `llms-full.txt`, the first source the research process checks.
    LlmsTxt,
    /// The vendor's human-readable documentation.
    Documentation,
    /// The protocol or API specification itself.
    Specification,
    /// A machine-readable API description: an `OpenAPI` document, a discovery document, or protobuf.
    ApiDescription,
    /// The vendor's official SDK repository.
    Sdk,
    /// Release notes or a changelog, the source that dates a breaking change.
    Changelog,
    /// The terms of service or developer policy a deployment is bound by.
    Terms,
    /// The vendor's rate-limit or quota documentation.
    ///
    /// Required by the document's own list ("rate-limit and quota documentation links") and separated from
    /// [`Self::Documentation`] because a quota page is what an operator checks when a connector starts
    /// failing, and it is usually not the page a browser lands on.
    RateLimits,
    /// The vendor's OAuth or authentication documentation.
    ///
    /// Separated for the same reason: `tools-and-connectors.md`'s OAuth section has nine specific
    /// requirements, and they are stated in a specific document that a general docs link does not reach.
    Authentication,
    /// The vendor's webhook, subscription, or push-notification documentation.
    Webhooks,
}

impl LinkKind {
    /// Returns the stable snake-case wire code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LlmsTxt => "llms_txt",
            Self::Documentation => "documentation",
            Self::Specification => "specification",
            Self::ApiDescription => "api_description",
            Self::Sdk => "sdk",
            Self::Changelog => "changelog",
            Self::Terms => "terms",
            Self::RateLimits => "rate_limits",
            Self::Authentication => "authentication",
            Self::Webhooks => "webhooks",
        }
    }

    /// Returns whether a connector declaring this link kind satisfies the research requirement.
    ///
    /// `LlmsTxt` counts because `external-research.md` lists it **first**, and a record that reads one is a
    /// record with a source. A [`Self::RateLimits`] or [`Self::Webhooks`] link alone does not: those pages
    /// describe one aspect and cannot substitute for the vendor's own contract.
    #[must_use]
    pub const fn satisfies_research_requirement(self) -> bool {
        matches!(
            self,
            Self::LlmsTxt | Self::Documentation | Self::Specification | Self::Sdk
        )
    }
}

/// A documentation link, with the kind of document it names.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentationLink {
    /// What kind of document this is.
    pub kind: LinkKind,
    /// The URL, which must be `https` (see [`DocumentationLinks::new`]).
    pub url: String,
    /// What the document is for, in the author's words. Bounded and non-empty, because a bare URL leaves a
    /// reader guessing which page answers which question.
    pub purpose: String,
}

/// The one line of context the manifest carries about the research behind it.
///
/// `docs/development/external-research.md` requires a dated record with "the exact URLs, versions, access
/// date, supported operations, authentication method, limits, and unresolved ambiguities". The record
/// itself lives in `docs/research/integrations/`, and this is the **pointer to it plus the date**, so a
/// manifest can be judged stale without opening the repository.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchRecord {
    /// The repository-relative path of the research record.
    pub path: String,
    /// The date the record was last verified, as `YYYY-MM-DD`.
    ///
    /// Kept as text rather than a parsed date, deliberately: this crate has no calendar and a manifest is
    /// read by an operator, not compared by arithmetic. It is validated as a shape, because `2026-9-7` and a
    /// paragraph are both "text" and neither is a date.
    pub last_verified: String,
}

impl ResearchRecord {
    /// Validates a research pointer.
    ///
    /// # Errors
    ///
    /// Returns [`ConnectorError::ResearchRecord`] when the path is empty or not repository-relative, or when
    /// the date is not `YYYY-MM-DD`.
    pub fn new(
        path: impl Into<String>,
        last_verified: impl Into<String>,
    ) -> Result<Self, ConnectorError> {
        let path = path.into();
        let last_verified = last_verified.into();
        if path.trim().is_empty()
            || path.starts_with('/')
            || path.starts_with('\\')
            || path.contains("..")
            || path.contains(':')
        {
            return Err(ConnectorError::ResearchRecord {
                reason: "the path must be repository-relative, with no leading separator, drive letter, or \
                         `..`",
            });
        }
        if !is_iso_date(&last_verified) {
            return Err(ConnectorError::ResearchRecord {
                reason: "`last_verified` must be a `YYYY-MM-DD` date",
            });
        }
        Ok(Self {
            path,
            last_verified,
        })
    }
}

/// Returns whether the text is `YYYY-MM-DD` with no time or zone.
///
/// Checked as a **shape** rather than as a calendar date: a manifest is a document a person reads, and this
/// crate has no calendar to validate against. `2026-13-45` would pass, and that is recorded rather than
/// hidden — the alternative is a date library in a crate that otherwise has no notion of time.
fn is_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                *byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
}

/// The documentation sources a connector declared.
///
/// Validated as a **set with a requirement**: `tools-and-connectors.md` lists "official docs, `llms.txt`,
/// OpenAPI/SDK, changelog, and terms links", and [`LinkKind::satisfies_research_requirement`] decides which
/// of those discharge the research obligation. A connector with links that are all rate-limit or terms pages
/// is refused, because it has not named its vendor's contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "Vec<DocumentationLink>", into = "Vec<DocumentationLink>")]
pub struct DocumentationLinks(Vec<DocumentationLink>);

impl DocumentationLinks {
    /// Validates a set of documentation links.
    ///
    /// # Errors
    ///
    /// Returns [`ConnectorError::Documentation`] when the list is empty, longer than
    /// [`MAX_MANIFEST_LINKS`], holds a duplicate kind, holds a `purpose` that is empty or oversized, holds a
    /// `url` that is not `https`, or **holds no link that discharges the research requirement**.
    ///
    /// # Why `https` and not merely "a URL"
    ///
    /// A manifest is read by an operator and its links are followed by a browser. An `http` link is a
    /// document that can be changed in transit, which matters because the document explains what a
    /// credential grant authorizes. `file:` and `javascript:` are refused by the same check rather than by a
    /// separate one, since neither has an authority component at all.
    pub fn new(links: Vec<DocumentationLink>) -> Result<Self, ConnectorError> {
        if links.is_empty() {
            return Err(ConnectorError::Documentation {
                reason: "a connector must declare at least one documentation link",
            });
        }
        if links.len() > MAX_MANIFEST_LINKS {
            return Err(ConnectorError::Documentation {
                reason: "a connector may declare at most 16 documentation links",
            });
        }
        let mut kinds = BTreeSet::new();
        let mut discharges_research = false;
        for link in &links {
            if !link.url.starts_with("https://") || link.url.len() > 512 {
                return Err(ConnectorError::Documentation {
                    reason: "every documentation link must be an `https://` URL of at most 512 characters",
                });
            }
            // A link with no stated purpose is a bare URL, and the reader cannot tell which question it
            // answers. Bounded above so a manifest cannot become an essay.
            let purpose = link.purpose.trim();
            if purpose.is_empty() || purpose.chars().count() > 200 {
                return Err(ConnectorError::Documentation {
                    reason: "every documentation link must state a purpose of 1 to 200 characters",
                });
            }
            if !kinds.insert(link.kind) {
                return Err(ConnectorError::Documentation {
                    reason: "a documentation kind may appear only once, because the second entry would \
                             silently override the first",
                });
            }
            discharges_research |= link.kind.satisfies_research_requirement();
        }
        if !discharges_research {
            return Err(ConnectorError::Documentation {
                reason: "at least one link must be the vendor's `llms.txt`, documentation, specification, or \
                         SDK; rate-limit, terms, authentication and webhook pages describe one aspect and \
                         cannot substitute for the contract",
            });
        }
        Ok(Self(links))
    }

    /// Returns the links.
    #[must_use]
    pub fn as_slice(&self) -> &[DocumentationLink] {
        &self.0
    }

    /// Returns whether a link of the given kind is present.
    #[must_use]
    pub fn has(&self, kind: LinkKind) -> bool {
        self.0.iter().any(|link| link.kind == kind)
    }
}

impl TryFrom<Vec<DocumentationLink>> for DocumentationLinks {
    type Error = ConnectorError;

    fn try_from(value: Vec<DocumentationLink>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<DocumentationLinks> for Vec<DocumentationLink> {
    fn from(value: DocumentationLinks) -> Self {
        value.0
    }
}

/// How the connector's data is classified, and where it may be processed.
///
/// `tools-and-connectors.md` lists "data classifications and residency notes" as manifest content, and
/// `security.md`'s table is the classification vocabulary. The **highest** classification a connector handles
/// is what matters for placement, so this is a single value rather than a per-operation map: a connector that
/// handles mail at all handles confidential content, and a reader asking "may this reach a remote model"
/// wants one answer rather than a table to reduce.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    /// Public content: public docs, a public catalogue.
    Public,
    /// Configuration metadata, non-sensitive activity.
    Internal,
    /// Email, calendar, documents, memories, transcripts.
    Confidential,
    /// Tokens, keys, passwords, signing material.
    ///
    /// A connector declaring this is saying it handles secret material directly, which is unusual — normally
    /// a credential goes to the secret store and the connector sees only a [`crate::SecretField`] reference.
    /// It is representable because a connector that *rotates* credentials genuinely does handle them.
    Secret,
    /// Health, financial, or legal data, recordings, and biometric-like voice data.
    Restricted,
}

impl Classification {
    /// Returns the stable snake-case wire code, matching `jarvis_core::Sensitivity`'s own codes.
    ///
    /// Deliberately the same strings: a connector's classification becomes a `Sensitivity` when its output
    /// enters a context, and two spellings for one level is the "two values that must agree, with nothing
    /// holding both" defect in a place where the disagreement would be a **privacy** bug rather than a
    /// cosmetic one.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Confidential => "confidential",
            Self::Secret => "secret",
            Self::Restricted => "restricted",
        }
    }

    /// Returns whether content at this level may leave the machine to a third-party model.
    ///
    /// `security.md` gives the rule per level and `jarvis_core::Sensitivity::can_flow_to` is the enforced
    /// form, so this mirrors it rather than re-deciding: `Internal` and below may reach a remote model,
    /// `Confidential` and above may not. Mirrored as a **predicate on the same enum** so the manifest can be
    /// read without loading `jarvis-core`, which is what "parseable without loading provider code" means —
    /// and it is asserted against `Sensitivity` in the tests so the two cannot drift.
    #[must_use]
    pub const fn may_reach_a_remote_model(self) -> bool {
        matches!(self, Self::Public | Self::Internal)
    }
}

/// Where a connector's data may be processed.
///
/// A residency note is a **statement the deployment is bound by** rather than a technical control: nothing
/// in this crate can enforce where a provider stores data. It is required because an operator choosing a
/// connector needs to know, and because `requirements.md`'s multi-user story has residency implications that
/// are otherwise invisible.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DataResidency {
    /// A short statement of where the provider processes and stores the data.
    ///
    /// Free text deliberately: residency regimes are legal facts about a vendor, and a closed vocabulary here
    /// would either be wrong or become a list this crate maintains. What matters is that the field is
    /// **required**, so an author must reach a conclusion rather than omit the question.
    pub note: String,
    /// Whether the deployment has verified this claim against the provider's own terms.
    ///
    /// Three-valued rather than a `bool`, mirroring `jarvis_models::Normalization`'s reasoning: an
    /// unestablished property must not be reported as one of two definite answers. `Unverified` is the
    /// default, because that is what an author who did not check should produce.
    #[serde(default)]
    pub verification: ResidencyVerification,
}

/// Whether a residency claim was checked against the provider's terms.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidencyVerification {
    /// Confirmed against the provider's published terms, with the link recorded.
    Verified,
    /// The provider's terms do not state a position, so nothing is claimed.
    NotStated,
    /// Nobody has checked.
    #[default]
    Unverified,
}

/// The JARVIS versions a connector claims compatibility with.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityStatus {
    /// The oldest JARVIS version this connector supports, as a dotted numeric version.
    pub minimum_jarvis_version: String,
    /// Whether the connector is known to work with the current platform.
    ///
    /// A **status** rather than a `bool` because "broken by a vendor change" and "not yet verified against
    /// this platform version" require different operator responses, and a `bool` would force them to read the
    /// same. `security.md`'s "missing or stale evidence fails closed" is why the permissive value is the
    /// narrow one.
    pub status: CompatibilityVerdict,
    /// What is known, when the status is not [`CompatibilityVerdict::Current`].
    pub note: Option<String>,
}

/// Whether a connector is known to work with the current platform.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompatibilityVerdict {
    /// Verified against the current platform.
    Current,
    /// Not yet verified. It may work; nothing claims it does.
    Unverified,
    /// A known incompatibility, with the reason in the note.
    Incompatible,
    /// The vendor changed the API and the connector has not caught up.
    VendorChange,
}

impl CompatibilityVerdict {
    /// Returns whether the connector may be installed.
    #[must_use]
    pub const fn is_installable(self) -> bool {
        matches!(self, Self::Current)
    }
}

/// A secret the deployment must supply, named but never valued.
///
/// # Why a field name and not a value
///
/// `security.md`: "Secret values are short-lived in memory, zeroized where practical, and passed only to the
/// adapter operation that requires them", and "no token material in model context, URLs/logs, diagnostics,
/// or normal database columns". A manifest is a document that is read at install time, displayed by
/// `doctor`, and possibly copied between machines, so a field able to hold token material would be one whose
/// content reached all of those. There is no `value` field in this struct, and that absence is the control.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SecretField {
    /// The name a configuration refers to this secret by.
    pub name: String,
    /// What kind of value it is, so a client can prompt sensibly and a validator can reject an obviously
    /// wrong paste.
    pub kind: SecretKind,
    /// Whether the connector works without it.
    ///
    /// Some providers issue an optional client secret; others require one. Both are real, so this is a
    /// `bool` — but the field is **required**, so an author must decide rather than omit an answer.
    pub required: bool,
    /// Why it is needed, in the author's words, for an operator deciding what to paste.
    pub purpose: String,
}

/// The kind of value a secret field expects.
///
/// A **closed vocabulary on purpose**, and each variant exists because it changes how a client handles the
/// value: a `ClientSecret` must never be shown back, a `SigningSecret` is compared rather than sent, and a
/// `RefreshToken` is what a rotation invalidates. A free-text `kind` would be a field nobody could validate,
/// which is the same reasoning `jarvis-tools`' `EffectSet` uses.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    /// An OAuth client secret.
    ClientSecret,
    /// A personal access token.
    PersonalAccessToken,
    /// A long-lived API key.
    ApiKey,
    /// A refresh token, which a rotation invalidates.
    RefreshToken,
    /// A webhook signing secret, **compared** rather than sent.
    SigningSecret,
    /// A service-account private key.
    PrivateKey,
    /// A device or installer pairing code, used once.
    PairingCode,
}

impl SecretKind {
    /// Returns the stable snake-case wire code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClientSecret => "client_secret",
            Self::PersonalAccessToken => "personal_access_token",
            Self::ApiKey => "api_key",
            Self::RefreshToken => "refresh_token",
            Self::SigningSecret => "signing_secret",
            Self::PrivateKey => "private_key",
            Self::PairingCode => "pairing_code",
        }
    }

    /// Returns whether the value is presented to the provider as a **credential** rather than compared.
    ///
    /// A signing secret is the exception, and the distinction matters to diagnostics: a credential is
    /// redacted everywhere, while a signing secret is additionally never *displayed* even to the operator
    /// who supplied it, because it authenticates the provider rather than the user.
    #[must_use]
    pub const fn is_presented(self) -> bool {
        !matches!(self, Self::SigningSecret)
    }
}

/// What the connector supports for receiving provider events.
///
/// Required rather than optional, and with an explicit [`Self::Unsupported`] variant: `tools-and-connectors.md`
/// lists "webhook and polling capabilities" as manifest content, and an absent field would be
/// indistinguishable from "the author did not consider it". `security.md`'s webhook threats (spoofing,
/// replay, wrong endpoint) are why a webhook declaration carries a binding rather than a bare URL.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WebhookSupport {
    /// The provider pushes events and the connector verifies them.
    Push {
        /// How the connector authenticates the sender.
        scheme: crate::webhook::SignatureScheme,
        /// How the connector binds a delivery to one account and one endpoint.
        binding: crate::webhook::WebhookBinding,
    },
    /// The connector polls, and the manifest states the interval as far as the provider documents it.
    Polling {
        /// The minimum polling interval, and whether the provider actually states one.
        ///
        /// Three-valued because a number here is a **claim about the provider**, and a bare `u32` forces a
        /// connector to either invent a figure or drop the capability. Google is the case that exposed it:
        /// the Gmail push guide recommends falling back to `history.list` after a quiet period and states no
        /// floor at all, so no honest `u32` exists. Same reasoning as `RateLimitEvidence`, where a limit
        /// "read as documented when it was guessed is a budget the deployment may plan around incorrectly".
        interval: PollingInterval,
    },
    /// The provider offers neither, so the connector is pull-only on demand.
    Unsupported,
}

/// The minimum polling interval a connector may use, and how well it is established.
///
/// The variants carry the number rather than sitting beside an `Option`, because a separate evidence field
/// would make `Some(3600)` beside `Unknown` representable — a figure and a disclaimer that contradict each
/// other, with nothing choosing between them. Attaching the number to the variant that has one makes the
/// contradiction unrepresentable, which is the move this crate makes everywhere else.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PollingInterval {
    /// The provider's own documentation states a minimum, and the manifest links that page.
    Documented(u32),
    /// A minimum established by observation, with nothing documented.
    ///
    /// Distinct from [`Self::Documented`] because the two carry different confidence: an observed floor can
    /// move with the provider's backend, and an operator planning a schedule needs to know which they have.
    Observed(u32),
    /// Nobody has established a minimum.
    ///
    /// **Not a refusal**, and the variant exists for a provider that genuinely has none to state. A connector
    /// declaring this is saying "I poll, and I will not claim a floor the provider never set" — which is more
    /// honest than either an invented number or dropping to [`WebhookSupport::Unsupported`], whose own doc
    /// says the provider offers neither.
    Unknown,
}

impl PollingInterval {
    /// Returns the interval in seconds, when one is established.
    #[must_use]
    pub const fn seconds(self) -> Option<u32> {
        match self {
            Self::Documented(seconds) | Self::Observed(seconds) => Some(seconds),
            Self::Unknown => None,
        }
    }

    /// Returns whether the provider's own documentation states the interval.
    ///
    /// The predicate a scheduler wants, and deliberately narrower than
    /// `seconds().is_some()`: a schedule built on an observed floor is a guess with evidence, not a
    /// documented limit, and a caller that cannot tell them apart will treat one as the other.
    #[must_use]
    pub const fn is_documented(self) -> bool {
        matches!(self, Self::Documented(_))
    }
}

/// One operation a connector can perform, with the effects it declares.
///
/// The operation's **effects are the manifest's most security-relevant content**: policy decides whether a
/// call may happen from a `ToolDefinition`, which is derived from these. `P3-008b` established the shape —
/// an external party's self-report must not be the input, because risk is a floor **raised** by effects and
/// a server that under-reports gets auto-approved work — and the same asymmetry holds here, where the
/// connector is third-party code talking to a third-party service.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorOperation {
    /// The operation's identifier, unique within the connector and usable as a tool name segment.
    pub id: String,
    /// What the operation does, for model selection and for an operator reading the manifest.
    pub description: String,
    /// The effects it declares.
    ///
    /// A `Vec` in the wire form and an [`EffectSet`] once validated, because a derived `Deserialize` on
    /// `EffectSet` would make an **empty** set representable — the one value the type exists to exclude. The
    /// same reasoning `jarvis-tools` records for `RetryDeclaration`.
    pub effects: Vec<ToolEffect>,
    /// The baseline risk, `0..=3`.
    ///
    /// A number in the wire form because that is what a manifest author writes; validated against the effect
    /// floor by [`ConnectorOperation::effect_set`]'s caller, which is where `risk >= floor` is checked.
    pub risk: u8,
    /// The JARVIS scopes a caller must hold to invoke it.
    pub required_scopes: Vec<String>,
    /// Whether the operation is **idempotent at the provider**.
    ///
    /// Three-valued, and `Unknown` is the default, because `AGENTS.md`'s rule ("idempotency where relevant")
    /// and `tools-and-connectors.md`'s "retry classification" both depend on this being right: retrying a
    /// non-idempotent effect is a second effect, and a `bool` would force "we did not check" to read as
    /// "safe to retry".
    #[serde(default)]
    pub idempotency: ProviderIdempotency,
    /// What the connector knows about the provider's rate limits, if anything.
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
}

/// Whether an operation is safe to repeat at the provider.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderIdempotency {
    /// The provider documents that a repeat is a no-op, or accepts a caller-supplied key.
    Declared,
    /// The provider accepts a caller-supplied idempotency key.
    ///
    /// Distinct from [`Self::Declared`] because the *caller* must then generate and reuse the key, which is
    /// `jarvis-tools`' `Idempotency::ProviderKey` — a different implementation obligation from "the provider
    /// makes it safe".
    ProviderKey,
    /// The provider does not document it, so a retry may be a second effect.
    #[default]
    Unknown,
    /// The provider documents that a repeat **duplicates** the effect.
    NotIdempotent,
}

impl ProviderIdempotency {
    /// Returns whether an automatic retry of this operation is permitted.
    ///
    /// `Unknown` refuses, which is the whole reason the type is three-valued: the question a retry asks is
    /// "is a second effect impossible", and an unanswered question must not be read as a yes.
    #[must_use]
    pub const fn permits_automatic_retry(self) -> bool {
        matches!(self, Self::Declared | Self::ProviderKey)
    }
}

/// An auth method the connector supports, with the scopes it needs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthMethodDeclaration {
    /// The method.
    pub method: AuthMethod,
    /// What the method is for, in the author's words.
    pub purpose: String,
    /// The scopes the deployment must request, in the **provider's own spelling**.
    ///
    /// The provider's spelling rather than JARVIS's, and that is a deliberate exception to the rule that
    /// JARVIS vocabulary is canonical: these strings are sent in an authorization request, so translating
    /// them into JARVIS scope names and back would be a lossy round trip in the one place a loss is a
    /// **wrong grant**. `jarvis-tools::Scope` remains what a *caller* must hold; this is what the *provider*
    /// must be asked for, and `P5-002` maps between them.
    pub scopes: Vec<String>,
    /// Whether the method can be used without also using another.
    ///
    /// Some providers require both a client secret and a refresh token; others offer alternatives. Required
    /// so an author decides, and the connector's `auth_methods` is validated to hold the method variants it
    /// needs (a `Pkce` flow without a client identifier is refused).
    pub required: bool,
}

/// A connector's manifest: everything an operator needs before installing it.
///
/// Constructed through [`ConnectorManifest::new`], which validates the cross-field rules a set of structs
/// cannot express on their own. `Deserialize` is then routed through the same constructor, so a manifest
/// loaded from a document is validated exactly as one built in code — the same arrangement
/// `jarvis-tools::ToolDefinition` uses, and for the same reason: a manifest that could be parsed without its
/// rules applied would be a way to install a connector nobody checked.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "ConnectorManifestParts", deny_unknown_fields)]
pub struct ConnectorManifest {
    id: ConnectorId,
    version: ConnectorVersion,
    display_name: String,
    provider: String,
    operations: Vec<ValidatedOperation>,
    auth_methods: Vec<AuthMethodDeclaration>,
    webhook: WebhookSupport,
    secret_fields: Vec<SecretField>,
    classification: Classification,
    residency: DataResidency,
    links: DocumentationLinks,
    research: ResearchRecord,
    compatibility: CompatibilityStatus,
}

/// An operation whose effects and risk have been checked against each other.
///
/// The distinction from [`ConnectorOperation`] is the point: a manifest holds only validated operations, so
/// "risk is at least the effect floor" is a property of the value rather than a check a reader performs.
/// `P3-001`'s reason for keeping `RetryPolicy` out of a deserializable type applies exactly.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ValidatedOperation {
    id: String,
    description: String,
    effects: EffectSet,
    risk: u8,
    required_scopes: Vec<String>,
    idempotency: ProviderIdempotency,
    rate_limit: Option<RateLimit>,
}

impl ValidatedOperation {
    /// Returns the operation identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the operation's description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the validated effect set.
    #[must_use]
    pub fn effects(&self) -> &EffectSet {
        &self.effects
    }

    /// Returns the baseline risk.
    #[must_use]
    pub fn risk(&self) -> u8 {
        self.risk
    }

    /// Returns the JARVIS scopes a caller must hold.
    #[must_use]
    pub fn required_scopes(&self) -> &[String] {
        &self.required_scopes
    }

    /// Returns whether an automatic retry is permitted.
    #[must_use]
    pub fn idempotency(&self) -> ProviderIdempotency {
        self.idempotency
    }

    /// Returns the declared rate limit, if any.
    #[must_use]
    pub fn rate_limit(&self) -> Option<RateLimit> {
        self.rate_limit
    }
}

/// The unchecked form a manifest is deserialized into.
///
/// Private and separate from [`ConnectorManifest`] so the public type cannot be built without validation,
/// while `Deserialize` still works. The alternative — validating in `Deserialize` and storing the result —
/// is not expressible for a struct with private fields without this split.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectorManifestParts {
    id: ConnectorId,
    version: ConnectorVersion,
    display_name: String,
    provider: String,
    operations: Vec<ConnectorOperation>,
    auth_methods: Vec<AuthMethodDeclaration>,
    webhook: WebhookSupport,
    secret_fields: Vec<SecretField>,
    classification: Classification,
    residency: DataResidency,
    links: DocumentationLinks,
    research: ResearchRecord,
    compatibility: CompatibilityStatus,
}

impl ConnectorManifest {
    /// Validates and records a manifest.
    ///
    /// # Errors
    ///
    /// Returns a [`ConnectorError`] for every cross-field rule this module documents: an empty or oversized
    /// name, no operations, a duplicate or unusable operation identifier, an empty or too-long scope list, a
    /// risk below the operation's effect floor, an operation that retries a non-idempotent effect, no auth
    /// method, a duplicate or unusable secret field, an unparseable JARVIS version, or a claim of
    /// compatibility with a version that predates the connector surface.
    #[allow(
        clippy::too_many_arguments,
        reason = "one parameter per manifest field, which is the shape the document's own list has; a struct would move the omission one level down rather than remove it"
    )]
    pub fn new(
        id: ConnectorId,
        version: ConnectorVersion,
        display_name: impl Into<String>,
        provider: impl Into<String>,
        operations: Vec<ConnectorOperation>,
        auth_methods: Vec<AuthMethodDeclaration>,
        webhook: WebhookSupport,
        secret_fields: Vec<SecretField>,
        classification: Classification,
        residency: DataResidency,
        links: DocumentationLinks,
        research: ResearchRecord,
        compatibility: CompatibilityStatus,
    ) -> Result<Self, ConnectorError> {
        let display_name = display_name.into();
        if display_name.trim().is_empty() || display_name.chars().count() > 100 {
            return Err(ConnectorError::DisplayName);
        }
        let provider = provider.into();
        if provider.trim().is_empty() || provider.chars().count() > 100 {
            return Err(ConnectorError::Provider);
        }
        let operations = validate_operations(operations)?;
        validate_auth_methods(&auth_methods)?;
        validate_secret_fields(&secret_fields)?;
        validate_compatibility(&compatibility)?;
        validate_classification(classification, &operations)?;
        validate_webhook(&webhook)?;
        Ok(Self {
            id,
            version,
            display_name,
            provider,
            operations,
            auth_methods,
            webhook,
            secret_fields,
            classification,
            residency,
            links,
            research,
            compatibility,
        })
    }

    /// Returns the connector identifier.
    #[must_use]
    pub fn id(&self) -> &ConnectorId {
        &self.id
    }

    /// Returns the manifest version.
    #[must_use]
    pub fn version(&self) -> &ConnectorVersion {
        &self.version
    }

    /// Returns the display name.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Returns the provider this connector talks to.
    #[must_use]
    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// Returns the validated operations.
    #[must_use]
    pub fn operations(&self) -> &[ValidatedOperation] {
        &self.operations
    }

    /// Returns the supported auth methods.
    #[must_use]
    pub fn auth_methods(&self) -> &[AuthMethodDeclaration] {
        &self.auth_methods
    }

    /// Returns the webhook or polling support.
    #[must_use]
    pub fn webhook(&self) -> &WebhookSupport {
        &self.webhook
    }

    /// Returns the secret fields a deployment must supply.
    #[must_use]
    pub fn secret_fields(&self) -> &[SecretField] {
        &self.secret_fields
    }

    /// Returns the highest classification the connector handles.
    #[must_use]
    pub fn classification(&self) -> Classification {
        self.classification
    }

    /// Returns the residency note.
    #[must_use]
    pub fn residency(&self) -> &DataResidency {
        &self.residency
    }

    /// Returns the documentation links.
    #[must_use]
    pub fn links(&self) -> &DocumentationLinks {
        &self.links
    }

    /// Returns the research record pointer.
    #[must_use]
    pub fn research(&self) -> &ResearchRecord {
        &self.research
    }

    /// Returns the compatibility status.
    #[must_use]
    pub fn compatibility(&self) -> &CompatibilityStatus {
        &self.compatibility
    }

    /// Returns the highest risk any declared operation carries.
    ///
    /// The value an operator actually needs: `tools-and-connectors.md`'s ladder is per operation, but "what
    /// is the worst thing this connector can be asked to do" is the question a grant decision asks. Derived
    /// rather than stored, so it cannot disagree with the operations it summarizes.
    #[must_use]
    pub fn highest_risk(&self) -> u8 {
        self.operations
            .iter()
            .map(ValidatedOperation::risk)
            .max()
            .unwrap_or(0)
    }

    /// Returns the highest classification among the operation effects, as a second opinion on a declaration.
    ///
    /// Used by the tests to prove the manifest's own [`Self::classification`] is not **lower** than what its
    /// operations imply — a connector that declares `Public` while one of its operations communicates
    /// externally is the under-reporting `P3-008b` warns about, in the field an operator trusts most.
    #[must_use]
    pub fn effect_floor(&self) -> u8 {
        self.operations
            .iter()
            .map(|operation| operation.effects.risk_floor())
            .max()
            .unwrap_or(0)
    }
}

/// Validates a connector's operation list.
///
/// # Why the risk floor is checked HERE
///
/// `tools-and-connectors.md`: "Context can raise risk but cannot lower a hard policy floor", and
/// `P3-001`/`P3-008b` both enforce `risk >= effects.risk_floor()` at construction. A manifest is the
/// **first** place an operation's risk is stated, so checking it here means an under-declared operation
/// cannot be installed at all — rather than being installed and then refused at registration, which would
/// report the refusal as a platform problem rather than a manifest defect.
fn validate_operations(
    operations: Vec<ConnectorOperation>,
) -> Result<Vec<ValidatedOperation>, ConnectorError> {
    if operations.is_empty() {
        return Err(ConnectorError::Operations {
            reason: "a connector must declare at least one operation",
        });
    }
    if operations.len() > MAX_CONNECTOR_OPERATIONS {
        return Err(ConnectorError::Operations {
            reason: "a connector may declare at most 64 operations",
        });
    }
    let mut identifiers = BTreeSet::new();
    let mut validated = Vec::with_capacity(operations.len());
    for operation in operations {
        if operation.id.is_empty()
            || operation.id.chars().count() > MAX_CONNECTOR_OPERATION_ID_CHARS
        {
            return Err(ConnectorError::Operations {
                reason: "every operation identifier must be 1 to 64 characters",
            });
        }
        // The same alphabet a `ToolId` name segment accepts, checked here so a connector cannot declare an
        // operation that could never become a tool.
        if !operation.id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        }) {
            return Err(ConnectorError::Operations {
                reason: "an operation identifier may hold only lowercase ASCII letters, digits, and `_`, \
                         because it becomes a tool name segment",
            });
        }
        if !identifiers.insert(operation.id.clone()) {
            return Err(ConnectorError::Operations {
                reason: "operation identifiers must be unique within a connector, because two operations \
                         with one name would make a call reach whichever was registered last",
            });
        }
        if operation.description.trim().is_empty() || operation.description.chars().count() > 512 {
            return Err(ConnectorError::Operations {
                reason: "every operation must have a description of 1 to 512 characters",
            });
        }
        if operation.required_scopes.len() > MAX_CONNECTOR_SCOPES {
            return Err(ConnectorError::Operations {
                reason: "an operation may require at most 64 scopes",
            });
        }
        // An effect set cannot be empty, and this is where that becomes a parse error rather than a
        // silently empty capability.
        let effects =
            EffectSet::new(operation.effects.clone()).ok_or(ConnectorError::Operations {
                reason: "every operation must declare at least one effect class; an empty set would mean \
                         `this does nothing`, which is not a representable statement about a provider call",
            })?;
        let floor = effects.risk_floor();
        if operation.risk < floor {
            return Err(ConnectorError::RiskBelowFloor {
                operation: operation.id,
                declared: operation.risk,
                floor,
            });
        }
        if operation.risk > jarvis_tools::MAX_RISK_LEVEL {
            return Err(ConnectorError::RiskAboveMaximum {
                operation: operation.id,
                declared: operation.risk,
                maximum: jarvis_tools::MAX_RISK_LEVEL,
            });
        }
        // The retry rule from `P3-001`: a blind retry of an ambiguous effect is a second effect. That rule is
        // **not** checked here, and the boundary is worth stating because it looks like an omission.
        //
        // A non-idempotent outward operation is legitimate and common — sending a mail is exactly that — so
        // refusing one here would refuse most connectors. What must not happen is a *retry policy* being
        // declared alongside a provider that duplicates the effect, and that pairing is not expressible in
        // this manifest at all: a tool's `RetryDeclaration` is derived in `P5-009`, where the operation
        // becomes a `ToolDefinition`, and **this type carries no retry policy to disagree with**. So there is
        // nothing to check here rather than a check that was forgotten.
        validated.push(ValidatedOperation {
            id: operation.id,
            description: operation.description,
            effects,
            risk: operation.risk,
            required_scopes: operation.required_scopes,
            idempotency: operation.idempotency,
            rate_limit: operation.rate_limit,
        });
    }
    Ok(validated)
}

/// Returns whether the effect set includes an effect that leaves the machine.
///
/// Used by the classification check, which is the **under-reporting** direction: a connector declaring
/// `Public` or `Internal` while one of its operations communicates externally would have its content treated
/// as safe for a remote model. That is `P3-008b`'s asymmetry in the field an operator trusts most, so it is a
/// constructor rule rather than a review note.
#[must_use]
fn has_outward_effect(effects: &EffectSet) -> bool {
    use jarvis_tools::ToolEffect;
    effects.contains(ToolEffect::ExternalCommunication)
        || effects.contains(ToolEffect::Write)
        || effects.contains(ToolEffect::Destructive)
        || effects.contains(ToolEffect::Financial)
}

/// Validates the declared auth methods.
fn validate_auth_methods(methods: &[AuthMethodDeclaration]) -> Result<(), ConnectorError> {
    if methods.is_empty() {
        return Err(ConnectorError::Auth {
            reason: "a connector must declare at least one auth method, or nothing could connect it",
        });
    }
    if !methods.iter().any(|method| method.required) {
        return Err(ConnectorError::Auth {
            reason: "at least one auth method must be marked required, because a connector with only \
                     optional methods has no way to connect",
        });
    }
    let mut seen = BTreeSet::new();
    for method in methods {
        if !seen.insert(method.method.tag()) {
            return Err(ConnectorError::Auth {
                reason: "an auth method may appear only once; a second declaration would silently override \
                         the first",
            });
        }
        if method.purpose.trim().is_empty() || method.purpose.chars().count() > 200 {
            return Err(ConnectorError::Auth {
                reason: "every auth method must state a purpose of 1 to 200 characters",
            });
        }
        if method.scopes.len() > MAX_CONNECTOR_SCOPES {
            return Err(ConnectorError::Auth {
                reason: "an auth method may request at most 64 scopes",
            });
        }
        if method.scopes.iter().any(|scope| scope.trim().is_empty()) {
            return Err(ConnectorError::Auth {
                reason: "a requested scope may not be empty, because an empty scope is one the provider \
                         cannot grant and the request would fail or be silently ignored",
            });
        }
    }
    Ok(())
}

/// Validates a connector's webhook declaration.
///
/// # Why `Push` has to validate its scheme at all
///
/// [`WebhookSupport::Push`] carries a [`SignatureScheme`](crate::webhook::SignatureScheme), and that scheme
/// may name [`SignatureAlgorithm::None`](crate::webhook::SignatureAlgorithm::None) — a delivery with **no**
/// authenticator. [`SignatureScheme::authenticates`](crate::webhook::SignatureScheme::authenticates) exists
/// to detect exactly that value, and its own documentation said the value "is refused by
/// [`WebhookBinding::new`]". It was not: `WebhookBinding::new` validates the *binding* — the path and the
/// account header or body field — and never sees the algorithm, which is a sibling field of the same enum
/// variant. So a connector could declare `Push` with an algorithm of `None`, and the platform would accept
/// unauthenticated writes from anyone who knew the path: the "webhook spoof" row of `security.md`'s threat
/// table with its control removed, declared in a manifest a reviewer would read as having provided one.
///
/// The refusal is here rather than in [`WebhookBinding::new`] because the two types are siblings and neither
/// owns the other; the manifest is the only object that holds both, and it is the one being validated.
fn validate_webhook(webhook: &WebhookSupport) -> Result<(), ConnectorError> {
    let unauthenticated = match webhook {
        WebhookSupport::Push { scheme, .. } => !scheme.authenticates(),
        // Neither of the other two carries a scheme, so neither can make a claim that needs checking.
        WebhookSupport::Polling { .. } | WebhookSupport::Unsupported => false,
    };
    if unauthenticated {
        return Err(ConnectorError::Webhook {
            reason: "a `push` declaration must name a signature algorithm that verifies the delivery, so \
                     `none` is refused: it would leave the endpoint accepting unauthenticated writes. A \
                     connector that cannot verify the sender should declare `polling` and be honest about it",
        });
    }
    Ok(())
}

/// Validates
/// Validates that a connector's declared classification covers its operations' effects.
///
/// # Why the under-reporting direction is the one that must be refused
///
/// A classification is what decides whether content may reach a remote model
/// ([`Classification::may_reach_a_remote_model`]) and what placement rule a deployment applies. So a connector
/// declaring `Public` while an operation communicates externally would have its content treated as safe to
/// leave the machine. The **over**-reporting direction is merely inconvenient, which is the same asymmetry
/// `P3-008b` records for risk: the ladder runs opposite to the incentive, so the check is on the declaration
/// rather than on trust.
///
/// The rule is deliberately not "the classification must equal the floor" — a mail connector legitimately
/// declares `Confidential` regardless of its effects, because what it *handles* is confidential. What is
/// refused is a classification **below** `Confidential` when an operation reaches outward, since that is the
/// combination that mislabels the content.
fn validate_classification(
    classification: Classification,
    operations: &[ValidatedOperation],
) -> Result<(), ConnectorError> {
    let reaches_outward = operations
        .iter()
        .any(|operation| has_outward_effect(operation.effects()));
    // `Secret` and `Restricted` are above `Confidential`, so only the two low levels can under-report.
    if reaches_outward
        && !matches!(
            classification,
            Classification::Confidential | Classification::Secret | Classification::Restricted
        )
    {
        return Err(ConnectorError::Classification {
            declared: classification.as_str(),
            reason: "a connector with an operation that communicates externally or writes must be classified \
                     at least `confidential`, because `internal` and below are the levels permitted to reach \
                     a remote model",
        });
    }
    Ok(())
}

/// Validates the declared secret fields.
fn validate_secret_fields(fields: &[SecretField]) -> Result<(), ConnectorError> {
    if fields.len() > MAX_CONNECTOR_SECRET_FIELDS {
        return Err(ConnectorError::Secrets {
            reason: "a connector may declare at most 16 secret fields",
        });
    }
    let mut names = BTreeSet::new();
    for field in fields {
        if field.name.is_empty()
            || field.name.len() > 64
            || !field
                .name
                .chars()
                .all(|character| character.is_ascii_lowercase() || character == '_')
        {
            return Err(ConnectorError::Secrets {
                reason: "a secret field name must be lowercase ASCII with `_`, 1 to 64 characters",
            });
        }
        if !names.insert(field.name.clone()) {
            return Err(ConnectorError::Secrets {
                reason: "secret field names must be unique; a duplicate would make a configuration supply \
                         one value for two purposes",
            });
        }
        if field.purpose.trim().is_empty() || field.purpose.chars().count() > 200 {
            return Err(ConnectorError::Secrets {
                reason: "every secret field must state a purpose of 1 to 200 characters, because an \
                         operator pasting a credential needs to know which one",
            });
        }
    }
    Ok(())
}

/// Validates the compatibility declaration.
/// Validates a compatibility claim.
///
/// `pub(crate)` rather than private because the scaffold generator checks a proposed minimum version through
/// it, so the `major.minor.patch` rule and the floor against [`MIN_SUPPORTED_JARVIS_VERSION`] exist in one
/// place. A scaffold that re-implemented the parse would be the second implementation of a rule a manifest is
/// actually built with.
pub(crate) fn validate_compatibility(
    compatibility: &CompatibilityStatus,
) -> Result<(), ConnectorError> {
    let minimum = &compatibility.minimum_jarvis_version;
    let parts: Vec<&str> = minimum.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || part.parse::<u32>().is_err())
    {
        return Err(ConnectorError::Compatibility {
            reason: "`minimum_jarvis_version` must be a `major.minor.patch` triple of numbers",
        });
    }
    if !is_at_least(minimum, MIN_SUPPORTED_JARVIS_VERSION) {
        return Err(ConnectorError::Compatibility {
            reason: "a connector may not claim compatibility below 0.5.0, the first version with a \
                     connector surface; the claim would read as working on an install that has none",
        });
    }
    // A verdict other than `Current` must say what is known, because "not verified" with no explanation is
    // indistinguishable from an author who forgot.
    if compatibility.status != CompatibilityVerdict::Current {
        let described = compatibility
            .note
            .as_deref()
            .map(str::trim)
            .is_some_and(|note| !note.is_empty() && note.chars().count() <= 500);
        if !described {
            return Err(ConnectorError::Compatibility {
                reason: "a compatibility verdict other than `current` must carry a note of 1 to 500 \
                         characters explaining what is known",
            });
        }
    }
    Ok(())
}

/// Returns whether `candidate` is at least `floor`, comparing dotted numeric triples.
///
/// Hand-written rather than a semver dependency: both values are validated `major.minor.patch` triples of
/// numbers, so the comparison is three integer comparisons. A version crate would be a dependency for
/// arithmetic this function already has to perform to validate the input.
fn is_at_least(candidate: &str, floor: &str) -> bool {
    let parse = |value: &str| -> Vec<u32> {
        value
            .split('.')
            .filter_map(|part| part.parse::<u32>().ok())
            .collect()
    };
    let candidate = parse(candidate);
    let floor = parse(floor);
    candidate >= floor
}

impl TryFrom<ConnectorManifestParts> for ConnectorManifest {
    type Error = ConnectorError;

    fn try_from(parts: ConnectorManifestParts) -> Result<Self, Self::Error> {
        Self::new(
            parts.id,
            parts.version,
            parts.display_name,
            parts.provider,
            parts.operations,
            parts.auth_methods,
            parts.webhook,
            parts.secret_fields,
            parts.classification,
            parts.residency,
            parts.links,
            parts.research,
            parts.compatibility,
        )
    }
}

/// Why a connector manifest or account is not usable.
///
/// Every variant names the **field** rather than the symptom, because the reader is an operator editing a
/// document. `P3-008i`'s lesson applies: "a configuration error message must not assert a cause it cannot
/// know", so none of these forwards a parser's text or a value that could echo the document.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ConnectorError {
    /// A connector identifier is unusable.
    #[error("the connector identifier `{value}` is unusable: {reason}")]
    Identifier {
        /// The offending value.
        value: String,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The webhook declaration would accept unauthenticated deliveries.
    ///
    /// A `push` connector whose signature algorithm is `none` has no control on its inbound path, so this
    /// refuses at manifest validation rather than at delivery time — where the only remaining signal would be
    /// traffic that already reached the handler.
    #[error("the connector's webhook declaration is unusable: {reason}")]
    Webhook {
        /// What is wrong.
        reason: &'static str,
    },
    /// A connector version is unusable.
    #[error(
        "the connector version is unusable: it must be 1 to 64 characters with no control characters"
    )]
    Version {
        /// The offending value.
        value: String,
    },
    /// The display name is unusable.
    #[error("a connector display name must be 1 to 100 characters")]
    DisplayName,
    /// The provider name is unusable.
    #[error("a connector provider name must be 1 to 100 characters")]
    Provider,
    /// The operation list is unusable.
    #[error("the connector's operations are unusable: {reason}")]
    Operations {
        /// What is wrong.
        reason: &'static str,
    },
    /// The declared classification under-reports what the connector's operations do.
    #[error("the connector declares `{declared}` classification, which is too low: {reason}")]
    Classification {
        /// What the manifest declared.
        declared: &'static str,
        /// Why it is too low.
        reason: &'static str,
    },
    /// An operation declares less risk than its effects require.
    ///
    /// The `P3-001` rule, checked at the first place an operation's risk is stated. `context can raise risk
    /// but cannot lower a hard policy floor`, so an under-declared operation would be auto-approved work.
    #[error(
        "the operation `{operation}` declares risk {declared} but its effects require at least {floor}"
    )]
    RiskBelowFloor {
        /// The offending operation.
        operation: String,
        /// What the manifest declared.
        declared: u8,
        /// What the effect set requires.
        floor: u8,
    },
    /// An operation declares a risk above the platform's maximum.
    #[error("the operation `{operation}` declares risk {declared}, above the maximum {maximum}")]
    RiskAboveMaximum {
        /// The offending operation.
        operation: String,
        /// What the manifest declared.
        declared: u8,
        /// [`jarvis_tools::MAX_RISK_LEVEL`].
        maximum: u8,
    },
    /// The auth methods are unusable.
    #[error("the connector's auth methods are unusable: {reason}")]
    Auth {
        /// What is wrong.
        reason: &'static str,
    },
    /// The secret fields are unusable.
    #[error("the connector's secret fields are unusable: {reason}")]
    Secrets {
        /// What is wrong.
        reason: &'static str,
    },
    /// The documentation links are unusable.
    #[error("the connector's documentation is unusable: {reason}")]
    Documentation {
        /// What is wrong.
        reason: &'static str,
    },
    /// The research record pointer is unusable.
    #[error("the connector's research record is unusable: {reason}")]
    ResearchRecord {
        /// What is wrong.
        reason: &'static str,
    },
    /// The compatibility declaration is unusable.
    #[error("the connector's compatibility declaration is unusable: {reason}")]
    Compatibility {
        /// What is wrong.
        reason: &'static str,
    },
}

#[cfg(test)]
#[path = "manifest_tests.rs"]
mod tests;
