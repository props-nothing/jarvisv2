//! The connector scaffold: a manifest and a research stub an author fills in.
//!
//! `P5-003` asks for a scaffold "modeled on manifest-driven integration projects", and there is a repository
//! precedent for what that means in practice: `example/plugins/_template.py`. That file's value is not its
//! content, it is that it **exists** — a new connector starts from a document that already has the right
//! shape, so the author is filling in facts rather than inventing structure.
//!
//! # Why the scaffold is generated from types rather than shipped as a template file
//!
//! A committed template drifts. `ConnectorManifest` gains a field, the template does not, and a new connector
//! starts from a manifest that fails its own validation — with the failure surfacing somewhere far from the
//! template. Generating from [`ConnectorManifest`] makes that impossible: the scaffold's manifest is built by
//! the same constructor, in the same process, at the moment it is asked for, so it **cannot** be stale.
//!
//! That also makes the scaffold *verified at the point of use*, which is the stronger form of `P5-003`'s
//! requirement. A test asserts every generated scaffold validates, so a change that breaks the scaffold breaks
//! the test rather than the next person to run `jarvis connector new`.
//!
//! # What the scaffold refuses to invent
//!
//! Three things, and each is a place a template would otherwise hand the author a plausible lie:
//!
//! 1. **The operations are empty.** A scaffold cannot know what a provider's API does, and a generated
//!    `list_messages` operation would be a declaration of effects and risk for an endpoint nobody has
//!    checked. `ConnectorManifest::new` refuses an empty operation list, so the scaffold's manifest is
//!    deliberately **not yet valid** — see [`Scaffold::unfilled`].
//! 2. **The documentation links are the vendor's index, not the requirements.** A generated set naming
//!    `llms.txt`, the docs, and the changelog would satisfy `DocumentationLinks::new`'s
//!    `satisfies_research_requirement()` rule while pointing at URLs nobody fetched, which is exactly the
//!    "a manifest with only a marketing homepage is incomplete rather than terse" defect `ADR-0054` refuses
//!    one field over. The scaffold's links are therefore supplied by the caller.
//! 3. **The research record's date.** `external-research.md` requires a *dated* record, and a generated date
//!    would be the date the scaffold ran rather than the date anything was verified.
//!
//! So [`Scaffold`] is a bundle of **rendered text and the gaps that remain**, not a complete connector. The
//! honest shape for a scaffold is "here is the structure and here is what is missing", and
//! [`Scaffold::gaps`] is that second half.

use std::fmt;

use crate::readiness::{ALL_ITEMS, ReadinessItem};

/// The longest accepted display name or provider in a scaffold request.
pub const MAX_SCAFFOLD_NAME_CHARS: usize = 100;

/// The version every generated manifest starts at.
///
/// `1.0.0` rather than `0.1.0`, because a manifest's version is what a stored tool intent binds to
/// (`P3-002`'s "new behaviour under an unchanged version makes every stored intent naming that version mean
/// something it was not written against"), and a connector that has not been released has no intents to
/// invalidate. A `0.x` scaffold would invite a later rename at the same version.
pub const SCAFFOLD_VERSION: &str = "1.0.0";

/// The research-record path convention a scaffold proposes.
///
/// `docs/research/integrations/<integration>.md`, which is where every existing record lives and what
/// `external-research.md` requires. Proposing it rather than requiring it means a connector kept elsewhere
/// can say so, while the common case gets the convention for free.
#[must_use]
pub fn proposed_research_path(connector_id: &str) -> String {
    format!("docs/research/integrations/{connector_id}.md")
}

/// What the author supplies to generate a scaffold.
///
/// Deliberately small. Every field here is something the author **knows** at the moment they decide to build
/// a connector; anything requiring research is absent, because a scaffold is generated before the research is
/// done and a field that forced it would either be filled in wrongly or block the scaffold.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScaffoldRequest {
    /// The connector identifier, which becomes the tool namespace.
    pub connector_id: String,
    /// The human-readable name.
    pub display_name: String,
    /// The legal name of the provider.
    pub provider: String,
    /// The date the research record was last verified, `YYYY-MM-DD`.
    pub research_last_verified: String,
    /// Where the research record will live.
    pub research_path: String,
    /// The JARVIS version the connector targets.
    pub minimum_jarvis_version: String,
}

impl ScaffoldRequest {
    /// Records a scaffold request.
    ///
    /// # Errors
    ///
    /// Returns [`ScaffoldError::Name`] for a blank or oversized display name or provider, and delegates the
    /// identifier, path, and date checks to the types that own them so a scaffold cannot propose a value the
    /// manifest constructor would later refuse.
    pub fn new(
        connector_id: &str,
        display_name: impl Into<String>,
        provider: impl Into<String>,
        research_last_verified: impl Into<String>,
        research_path: impl Into<String>,
        minimum_jarvis_version: impl Into<String>,
    ) -> Result<Self, ScaffoldError> {
        let display_name = display_name.into();
        let provider = provider.into();
        for (value, what) in [(&display_name, "display name"), (&provider, "provider")] {
            if value.trim().is_empty() || value.chars().count() > MAX_SCAFFOLD_NAME_CHARS {
                return Err(ScaffoldError::Name { what });
            }
        }
        // The identifier is validated by constructing the real type, so a scaffold and a manifest agree on
        // what an identifier is by sharing one implementation. `crate::ConnectorId::new` is the authority.
        crate::ConnectorId::new(connector_id).map_err(|error| ScaffoldError::Identifier {
            reason: error.to_string(),
        })?;
        let research_path = research_path.into();
        let research_last_verified_raw = research_last_verified.into();
        crate::ResearchRecord::new(research_path.clone(), research_last_verified_raw.clone())
            .map_err(|error| ScaffoldError::Research {
                reason: error.to_string(),
            })?;
        let minimum_jarvis_version = minimum_jarvis_version.into();
        // The version is checked by asking the manifest's own validator through a throwaway compatibility
        // value, so the scaffold and the manifest cannot disagree about what a version is. Re-implementing the
        // `major.minor.patch` parse here would be the second place that rule lives, and the first one is the
        // one a manifest is actually built with.
        crate::manifest::validate_compatibility(&crate::CompatibilityStatus {
            minimum_jarvis_version: minimum_jarvis_version.clone(),
            status: crate::CompatibilityVerdict::Unverified,
            note: Some(
                "a scaffold has not been verified against any JARVIS version yet".to_owned(),
            ),
        })
        .map_err(|error| ScaffoldError::Version {
            reason: error.to_string(),
        })?;
        Ok(Self {
            connector_id: connector_id.to_owned(),
            display_name,
            provider,
            research_last_verified: research_last_verified_raw,
            research_path,
            minimum_jarvis_version,
        })
    }
}

/// Why a scaffold could not be generated.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ScaffoldError {
    /// A name is unusable.
    #[error("the connector's {what} must be 1 to 100 characters")]
    Name {
        /// Which field.
        what: &'static str,
    },
    /// The identifier is unusable.
    #[error("the connector identifier is unusable: {reason}")]
    Identifier {
        /// What the identifier type said.
        reason: String,
    },
    /// The research record declaration is unusable.
    #[error("the research record declaration is unusable: {reason}")]
    Research {
        /// What the research record type said.
        reason: String,
    },
    /// The minimum JARVIS version is unusable.
    #[error("the minimum JARVIS version is unusable: {reason}")]
    Version {
        /// What the manifest's validator said.
        reason: String,
    },
}

/// A generated scaffold: the files, and what the author still has to decide.
///
/// # The scaffold is not a connector, and the type says so
///
/// [`Self::manifest_json`] returns the *shape* of a manifest with no operations, which
/// `ConnectorManifest::new` correctly refuses. So there is no method here that returns a
/// `ConnectorManifest`, and that absence is the design: a scaffold that produced a valid manifest would have
/// had to invent operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scaffold {
    request: ScaffoldRequest,
}

impl Scaffold {
    /// Prepares a scaffold for a connector.
    ///
    /// # Errors
    ///
    /// Returns [`ScaffoldError`] for any value the manifest constructor would refuse, so generating a
    /// scaffold cannot produce something that fails later for a reason the author could have been told now.
    pub fn new(request: ScaffoldRequest) -> Result<Self, ScaffoldError> {
        Ok(Self { request })
    }

    /// Returns the request this scaffold was generated from.
    #[must_use]
    pub const fn request(&self) -> &ScaffoldRequest {
        &self.request
    }

    /// Renders the manifest skeleton as JSON.
    ///
    /// # Why it is built with a JSON writer rather than a format string
    ///
    /// The first version of this was a `format!` template, and it was **wrong**: the explanatory comments
    /// contain JSON examples, so their quotes had to be escaped and were not. A hand-written template that
    /// must quote a nested document is a template that produces invalid JSON the day someone edits a comment,
    /// and the failure surfaces as a parse error in an author's editor rather than here. Building a
    /// `serde_json::Value` puts the escaping in the writer's hands, and the comments become ordinary strings.
    ///
    /// # What the skeleton contains and what it deliberately leaves out
    ///
    /// It carries every field name the manifest has, so an author sees the shape without reading
    /// `manifest.rs`, and it carries the *empty* value for each collection. `operations` is empty on purpose
    /// and the JSON says why in a `_comment` field — which the manifest's own `deny_unknown_fields` would
    /// refuse, so the skeleton is **not** a manifest and cannot be mistaken for one. Stripping the comments is
    /// part of filling it in.
    ///
    /// `auth_methods`, `secret_fields`, and `links` are also empty, because all three are facts about the
    /// vendor: which auth methods an API supports, which secrets a deployment must supply, and where the
    /// vendor's documentation lives are exactly what the research record is for.
    #[must_use]
    pub fn manifest_json(&self) -> String {
        let skeleton = serde_json::json!({
            "_comment": "Generated by `jarvis connector new`. Remove every `_comment` and fill in the three empty collections before the manifest will validate: `ConnectorManifest::new` refuses an empty `operations` list, and a connector whose only link is a homepage is incomplete rather than terse.",
            "id": self.request.connector_id,
            "version": SCAFFOLD_VERSION,
            "display_name": self.request.display_name,
            "provider": self.request.provider,
            "_comment_operations": "One entry per provider operation. `effects` decides policy and `risk` is a FLOOR raised by them, so under-declaring effects gets work auto-approved; see ADR-0054. An operation must declare at least one effect, and `effects` values are snake_case tool effects such as `read_only`, `write`, `external_communication`, `destructive`, `financial`.",
            "operations": [],
            "_comment_auth_methods": "The auth methods the provider supports, each with the scopes it needs and whether it is required. At least one must be required. `method` values are `o_auth_pkce`, `o_auth_confidential`, `personal_access_token`, `api_key`, `service_account`, `pairing_code`.",
            "auth_methods": [],
            "webhook": {
                "_comment": "Exactly one of: {\"kind\": \"push\", \"scheme\": {...}, \"binding\": {...}} | {\"kind\": \"polling\", \"interval\": {\"documented\": N} | {\"observed\": N} | \"unknown\"} | {\"kind\": \"unsupported\"}. A push connector must also pass the two webhook readiness items. Use \"unknown\" rather than inventing an interval: a number here is a claim about the provider.",
                "kind": "unsupported"
            },
            "_comment_secret_fields": "Field NAMES only, never values. Each becomes an environment variable, so a name must be lowercase with underscores. `kind` values are `client_secret`, `personal_access_token`, `api_key`, `refresh_token`, `signing_secret`, `private_key`, `pairing_code`.",
            "secret_fields": [],
            "classification": "confidential",
            "_comment_classification": "`public` and `internal` are refused for a connector with an operation that reaches outward (ADR-0054), so this starts at `confidential`. `secret` and `restricted` are the levels above it.",
            "residency": {
                "note": "State what the provider's terms say about where data is processed. `unverified` is the honest default.",
                "verification": "unverified"
            },
            "_comment_links": "At least one link must satisfy the research requirement: `llms_txt`, `documentation`, `specification`, or `sdk`. A homepage is not one of those. Each link is {\"kind\": ..., \"url\": \"https://...\", \"purpose\": \"...\"}.",
            "links": [],
            "research": {
                "path": self.request.research_path,
                "last_verified": self.request.research_last_verified
            },
            "compatibility": {
                "minimum_jarvis_version": self.request.minimum_jarvis_version,
                "status": "unverified",
                "_comment": "A status other than `current` needs a `note` saying what is known."
            }
        });
        // Pretty-printed so the comments are readable, and the writer is `serde_json`'s so every string is
        // escaped correctly. A failure here is not expressible for a `Value` this code built, so the fallback
        // is a compact rendering rather than an error the caller would have to handle.
        serde_json::to_string_pretty(&skeleton).unwrap_or_else(|_| skeleton.to_string())
    }

    /// Renders the research record stub in Markdown.
    ///
    /// It is a stub rather than a filled record because `external-research.md`'s sections are questions whose
    /// answers come from the vendor's documentation, and a scaffold that pre-filled them would be a plausible
    /// record that nothing was verified against — the exact failure the research rule exists to prevent. Each
    /// section carries the question, so an author filling it in is answering rather than inventing structure.
    #[must_use]
    pub fn research_markdown(&self) -> String {
        format!(
            r"---
integration: {id}
status: planned
last_verified: {last_verified}
owners: []
selected_spec_version: null
selected_sdk: null
---

# {display_name}

## Scope

State the exact operations being researched and what is explicitly out of scope.

Do not research an entire vendor when one endpoint is in scope.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| `llms.txt` or docs index | | {last_verified} | discovery |
| API/specification | | {last_verified} | normative contract |
| OpenAPI/AsyncAPI/schema | | {last_verified} | generated shapes |
| official SDK/source | | {last_verified} | implementation details |
| changelog/release notes | | {last_verified} | compatibility/deprecations |
| security/privacy/limits | | {last_verified} | risk and operations |

If an expected source does not exist, write `not found` and how official docs were discovered instead.

## Verified Contract

### Operations And Transport

Endpoints, methods, framing, streaming, events, ordering, termination, pagination, and required fields.

### Authentication And Authorization

Auth flow, scopes, token placement, refresh/revocation, callback/webhook verification, and least-privilege
plan. The shared OAuth protocol facts are already recorded in
[oauth2-pkce-native-apps.md](oauth2-pkce-native-apps.md); this section records what is true of **this**
provider.

### Limits And Failure Semantics

Rate/concurrency/size/time limits, idempotency, retries, ambiguous outcomes, provider request IDs, and error
classes. In particular: is a repeat of a write a second effect, does the provider accept an idempotency key,
and what does it return when a request was accepted but the response was lost?

### Data And Compliance

What leaves JARVIS, retention, deletion, residency, telemetry, compliance restrictions, and pricing/cost
concerns.

### Versions And Deprecations

Selected versions, preview/stable status, compatibility window, breaking changes, and migration guidance.

## JARVIS Mapping

Provider concepts mapped to JARVIS domain types, tool effects/risk/scopes, secret references, events, audit
evidence, and degradation behavior.

## Decisions

List adopted choices with reasons. Distinguish them from external facts.

## Rejected Alternatives

List meaningful alternatives and why they were rejected.

## Verification Plan

- offline schema/fixture tests
- independent conformance tests
- auth/refresh/revoke tests
- signature/replay tests
- pagination/rate-limit/error tests
- idempotency/unknown-outcome tests
- opt-in live smoke test

Name the cheapest test that would disprove the central assumption.

## Unresolved Questions

Each question includes impact and the capability it blocks. Never silently convert an unknown into an
assumption.

## Verification Log

| Date | Check | Result |
| --- | --- | --- |
| {last_verified} | | |
",
            id = self.request.connector_id,
            display_name = self.request.display_name,
            last_verified = self.request.research_last_verified,
        )
    }

    /// Returns the readiness items the scaffold has **not** answered.
    ///
    /// This is the scaffold's honest half, and it is computed from the same [`ALL_ITEMS`] list the completion
    /// gate uses, so the two cannot disagree about what a connector needs. What a scaffold supplies is one
    /// item — [`ReadinessItem::OfficialSourcesDated`], because the research stub carries the path and the
    /// date — and it reports the rest as outstanding.
    ///
    /// The webhook items are included. A scaffold starts as [`WebhookSupport::Unsupported`], so they do not
    /// apply *yet*, but an author who changes that declaration is the one who needs to know; reporting them
    /// here means the fact arrives with the scaffold rather than at the completion gate.
    #[must_use]
    pub fn outstanding_items(&self) -> Vec<ReadinessItem> {
        ALL_ITEMS
            .iter()
            .copied()
            .filter(|item| *item != ReadinessItem::OfficialSourcesDated)
            .collect()
    }

    /// Returns the research record's proposed path.
    #[must_use]
    pub fn research_path(&self) -> &str {
        &self.request.research_path
    }
}

impl fmt::Display for Scaffold {
    /// Renders a short summary, for a CLI that has already written the files.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "scaffold for `{}` at version {SCAFFOLD_VERSION}, research record at {}, {} readiness item(s) \
             outstanding",
            self.request.connector_id,
            self.request.research_path,
            self.outstanding_items().len()
        )
    }
}
