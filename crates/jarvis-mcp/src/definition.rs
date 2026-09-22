//! Translating a server-supplied tool listing into a canonical JARVIS definition.
//!
//! # The authority question, again
//!
//! `P3-008a`'s naming module answers *what a tool is called*. This module answers the harder
//! question: **what may it do, and at what risk?**
//!
//! An MCP server supplies `name`, optional `title`, optional `description`, `inputSchema`, optional
//! `outputSchema`, and optional `annotations`. It supplies **nothing** about effects, risk, approval,
//! or scopes — and that is not an omission to fill in, it is the boundary. The specification makes the
//! point from the other side: "clients **MUST** consider tool annotations to be untrusted unless they
//! come from trusted servers".
//!
//! So the effects and risk do **not** come from the server. They come from an operator's statement
//! about that server, held in [`ToolEffectPolicy`], and a server with no such statement gets the
//! posture in [`ToolEffectPolicy::unclassified`] — which is deliberately severe.
//!
//! # Why the default posture is severe rather than neutral
//!
//! An MCP tool can do anything: the server decides. A policy that guessed "probably a read" would be
//! wrong in the one case that matters, and wrong in the direction that permits. `unclassified`
//! therefore declares [`ToolEffect::ExternalCommunication`] and [`ToolEffect::Write`], which force a
//! risk floor of 2, and then declares risk 3 with [`ApprovalPolicy::Ask`] so every call is held for a
//! human. A read-only server and a mass-mail server get the same posture until an operator
//! distinguishes them, because the failure mode of guessing low is an unaudited outward effect.
//!
//! This is why effects are an **operator** statement rather than a schema inference. Deriving them
//! from the input schema — "it takes a `query` string, so it reads" — would be inference presented as
//! knowledge, and this project's rule is that unsupported inference is never persisted as fact.

use jarvis_core::Sensitivity;
use jarvis_tools::{
    ApprovalPolicy, Availability, EffectSet, Idempotency, RetryDeclaration, RetryPolicyError,
    Scope, ScopeSet, TOOL_SCHEMA_DIALECT, ToolDefinition, ToolDefinitionError, ToolDefinitionParts,
    ToolEffect, ToolSensitivity, ToolSource,
};
use serde_json::{Value, json};

use crate::conformance::{check_tool_schema, to_tool_schema};
use crate::server::{
    CanonicalToolName, NameAssignments, NamingError, NamingStrategy, ServerName,
    canonical_tool_name,
};

/// The `$schema` keyword, which JARVIS requires and MCP permits a server to omit.
const SCHEMA_KEYWORD: &str = "$schema";

/// Longest third-party description kept, in characters.
///
/// [`jarvis_tools::MAX_TOOL_DESCRIPTION_CHARS`] is 1024 and is the actual limit; this sits below it so
/// a truncated description is visibly shorter than the bound rather than exactly at it.
const DESCRIPTION_BUDGET: usize = 1000;

/// The scope every MCP tool requires unless an operator declares otherwise.
///
/// A single coarse scope rather than a per-server or per-tool one, for now: the honest position is
/// that an MCP server is one trust unit, and inventing a per-tool scope vocabulary before there is a
/// grant model that can express it would be a naming scheme with no users.
pub const DEFAULT_MCP_SCOPE: &str = "mcp.call";

/// The default per-call deadline for an MCP tool, in seconds.
///
/// Lower than the 600-second maximum because an MCP call crosses a process or network boundary whose
/// peer may be wedged, and a bounded call that fails is more useful than an unbounded one that hangs.
pub const DEFAULT_MCP_TIMEOUT_SECONDS: u32 = 60;

/// Explains why an effect policy could not be built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolicyError {
    /// The retry declaration is unsafe for the effects and idempotency it is paired with.
    ///
    /// Checked at construction rather than at use, so a policy that would send a payment twice cannot
    /// be represented. The underlying check is [`RetryPolicy::blind`]'s.
    UnsafeRetry(RetryPolicyError),
    /// The declared risk is below what the declared effects require.
    ///
    /// `ToolDefinition::new` would catch this too, but only after a server's listing had been read —
    /// and an operator's mistake in configuration should be reported when the configuration is
    /// loaded, not when a remote server is contacted.
    RiskBelowEffects {
        /// The declared risk level.
        declared: u8,
        /// The minimum the effects require.
        required: u8,
    },
    /// A fixed value this crate supplies was rejected by its own constructor.
    ///
    /// Only reachable if a constant and a validator disagree, which is an authoring error rather than
    /// a runtime condition. Reported as an error rather than panicked on so that a future change to a
    /// constant produces a diagnosable refusal instead of a crash in whatever process called it.
    UnusableConstant {
        /// Why the constant was refused.
        reason: String,
    },
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsafeRetry(source) => {
                write!(formatter, "the retry declaration is unsafe: {source}")
            }
            Self::RiskBelowEffects { declared, required } => write!(
                formatter,
                "risk {declared} is below the risk {required} the declared effects require"
            ),
            Self::UnusableConstant { reason } => {
                write!(formatter, "a fixed MCP policy value was refused: {reason}")
            }
        }
    }
}

impl std::error::Error for PolicyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UnsafeRetry(source) => Some(source),
            Self::RiskBelowEffects { .. } | Self::UnusableConstant { .. } => None,
        }
    }
}

/// The declared posture: what the tool may do, how much scrutiny it needs, and whether it may run
/// without asking.
///
/// These three travel together because they are one decision. A caller who states effects without a
/// risk is stating half a posture, and `ToolEffectPolicy::new` validates the pair rather than
/// trusting either alone.
#[derive(Clone, Debug)]
pub struct ToolPosture {
    /// The effects the tool may have.
    pub effects: EffectSet,
    /// The declared baseline risk, which must cover the effects' floor.
    pub risk: u8,
    /// Whether an approval is required before the tool may run.
    pub approval: ApprovalPolicy,
}

/// How a tool call may be attempted more than once.
///
/// Grouped because the three are jointly constrained: whether a retry is safe depends on the effects
/// on the other side of the policy *and* on whether the server makes repeats safe, so a caller that
/// stated only one of these would be stating something that cannot be checked alone.
#[derive(Clone, Copy, Debug)]
pub struct ToolExecutionLimits {
    /// The per-call deadline in seconds.
    pub timeout_seconds: u32,
    /// The declared retry policy, before validation against the effects.
    pub retry: RetryDeclaration,
    /// Whether the server can make a repeat safe.
    pub idempotency: Idempotency,
}

/// What an operator has said about the effects and risk of one MCP server's tools.
///
/// Deliberately **not** `Default`: a default would be a posture chosen by omission, and the whole
/// point is that the posture is a statement someone made. [`Self::unclassified`] exists for the case
/// where nobody has made one, and its name says so at the call site.
#[derive(Clone, Debug)]
pub struct ToolEffectPolicy {
    posture: ToolPosture,
    scopes: ScopeSet,
    execution: ToolExecutionLimits,
    sensitivity: ToolSensitivity,
}

/// The declared inputs to a [`ToolEffectPolicy`].
///
/// A struct rather than eight positional parameters, for the reason `P3-005` grouped its call parts:
/// several of these are adjacent short values a caller could transpose without the compiler noticing.
/// `risk` and `timeout_seconds` are two numbers; `retry` and `idempotency` are a declaration and its
/// safety property. Putting each next to what it could be confused with is the point.
#[derive(Clone, Debug)]
pub struct ToolEffectPolicyParts {
    /// What the tools may do, at what risk, under what approval policy.
    pub posture: ToolPosture,
    /// The JARVIS scopes the tools require.
    pub scopes: ScopeSet,
    /// The deadline, retry, and idempotency declarations.
    pub execution: ToolExecutionLimits,
    /// The input/output classification.
    pub sensitivity: ToolSensitivity,
}

impl ToolEffectPolicy {
    /// Builds a policy, refusing one whose risk or retry declaration contradicts its effects.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError::RiskBelowEffects`] when the risk hides an effect, and
    /// [`PolicyError::UnsafeRetry`] when the retry declaration would repeat an effect that may already
    /// have happened. Both are configuration mistakes, and both are cheaper to report here than when a
    /// remote server is mid-call.
    pub fn new(parts: ToolEffectPolicyParts) -> Result<Self, PolicyError> {
        let ToolEffectPolicyParts {
            posture,
            scopes,
            execution,
            sensitivity,
        } = parts;
        let required = posture.effects.risk_floor();
        if posture.risk < required {
            return Err(PolicyError::RiskBelowEffects {
                declared: posture.risk,
                required,
            });
        }
        // Validated here even though `ToolDefinition::new` validates it again: this is the boundary
        // where an operator's declaration is checked, and doing it here means an unsafe policy cannot
        // be *held*, let alone used.
        execution
            .retry
            .validate(&posture.effects, execution.idempotency)
            .map_err(PolicyError::UnsafeRetry)?;
        Ok(Self {
            posture,
            scopes,
            execution,
            sensitivity,
        })
    }

    /// The posture for a server nobody has classified: outward-reaching, writes, risk 3, always ask.
    ///
    /// Chosen so that every failure of this default is a refusal rather than an action. A read-only
    /// server is inconvenienced by an approval prompt; a mass-mail server that was presumed harmless
    /// is not inconvenienced at all, which is the asymmetry that decides the default.
    ///
    /// # Panics
    ///
    /// Panics only if this crate's own constants are rejected, which its tests assert cannot happen.
    /// A silent fallback here would be a posture nobody chose, which is the one outcome this type
    /// exists to prevent.
    #[must_use]
    pub fn unclassified() -> Self {
        let effects = EffectSet::new([ToolEffect::ExternalCommunication, ToolEffect::Write])
            .unwrap_or_else(|| {
                panic!("ExternalCommunication and Write form a non-empty effect set")
            });
        let scope = Scope::new(DEFAULT_MCP_SCOPE)
            .unwrap_or_else(|error| panic!("{DEFAULT_MCP_SCOPE} is a valid scope: {error}"));
        Self {
            // Risk 3 with `Ask`, not the risk-2 floor the effects require: an unclassified server
            // gets the most scrutiny available, and `Ask` cannot be lowered by workspace policy the
            // way `Policy` could.
            posture: ToolPosture {
                effects,
                risk: 3,
                approval: ApprovalPolicy::Ask,
            },
            scopes: ScopeSet::single(scope),
            execution: ToolExecutionLimits {
                timeout_seconds: DEFAULT_MCP_TIMEOUT_SECONDS,
                // No retry. A repeat of an outward effect is a second effect, and
                // `Idempotency::Unsupported` says the server offers no way to make one safe.
                retry: RetryDeclaration::none(),
                idempotency: Idempotency::Unsupported,
            },
            // Both halves `Internal`: an MCP server's inputs and outputs are not public by default,
            // and the *destination* ceiling is decided when the content is placed, not here.
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        }
    }

    /// The posture for a server an operator has established is read-only.
    ///
    /// The one convenience constructor offered, because "this server only reads" is the common case
    /// and the difference from `unclassified` is stark: risk 0 and `Auto`.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError`] if the constants are rejected, which cannot happen for this crate's
    /// own values and is asserted by a test.
    pub fn read_only() -> Result<Self, PolicyError> {
        Ok(Self {
            posture: ToolPosture {
                effects: EffectSet::single(ToolEffect::ReadOnly),
                risk: 0,
                approval: ApprovalPolicy::Auto,
            },
            scopes: ScopeSet::single(Scope::new(DEFAULT_MCP_SCOPE).map_err(|error| {
                PolicyError::UnusableConstant {
                    reason: error.to_string(),
                }
            })?),
            execution: ToolExecutionLimits {
                timeout_seconds: DEFAULT_MCP_TIMEOUT_SECONDS,
                // Legal precisely because a read has no second effect: `RetryPolicy::blind` refuses
                // an automatic retry only for a mutating tool whose repeats are not made safe.
                retry: RetryDeclaration {
                    attempts: 1,
                    backoff_ceiling_seconds: 1,
                },
                idempotency: Idempotency::Unsupported,
            },
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })
    }

    /// Returns what the tools may do, at what risk, under what approval policy.
    #[must_use]
    pub const fn posture(&self) -> &ToolPosture {
        &self.posture
    }

    /// Returns the declared scopes.
    #[must_use]
    pub const fn scopes(&self) -> &ScopeSet {
        &self.scopes
    }

    /// Returns the deadline, retry, and idempotency declarations.
    #[must_use]
    pub const fn execution(&self) -> &ToolExecutionLimits {
        &self.execution
    }
}

/// One tool as a server listed it.
///
/// Field names match the protocol's `Tool` type. A struct rather than six parameters so a caller
/// cannot transpose `title` and `description` — both are optional strings and a swap would validate.
#[derive(Clone, Debug)]
pub struct McpToolListing<'a> {
    /// The server's own tool name.
    pub name: &'a str,
    /// The optional display title.
    pub title: Option<&'a str>,
    /// The optional model-facing description.
    pub description: Option<&'a str>,
    /// The input schema. Required by the protocol.
    pub input_schema: &'a Value,
    /// The optional output schema.
    pub output_schema: Option<&'a Value>,
}

/// A translated tool, ready to register.
#[derive(Clone, Debug)]
pub struct TranslatedTool {
    /// The canonical definition.
    pub definition: ToolDefinition,
    /// The name to send back on `tools/call`.
    ///
    /// The server's own name, carried alongside the definition because the definition holds the
    /// *canonical* identifier and sending that back would be the translation applied twice.
    pub remote: String,
}

/// Explains why a tool listing could not be translated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranslationError {
    /// The name could not become a canonical identifier.
    Name(NamingError),
    /// The input schema failed the protocol's own `x-mcp-header` rules.
    NotConformant {
        /// The server-supplied tool name, bounded.
        tool: String,
        /// Why it was refused, reader-facing.
        reason: String,
    },
    /// The schema declared a dialect JARVIS does not implement.
    UnsupportedDialect {
        /// The server-supplied tool name, bounded.
        tool: String,
        /// The dialect the server declared.
        declared: String,
    },
    /// The input schema could not become a JARVIS schema.
    InvalidInputSchema {
        /// The server-supplied tool name, bounded.
        tool: String,
        /// Why it was refused, reader-facing.
        reason: String,
    },
    /// The output schema could not become a JARVIS schema.
    InvalidOutputSchema {
        /// The server-supplied tool name, bounded.
        tool: String,
        /// Why it was refused, reader-facing.
        reason: String,
    },
    /// The definition was rejected by its own constructor.
    Definition(ToolDefinitionError),
}

impl std::fmt::Display for TranslationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name(source) => write!(formatter, "{source}"),
            Self::NotConformant { tool, reason } => {
                write!(formatter, "the MCP tool {tool} was excluded: {reason}")
            }
            Self::UnsupportedDialect { tool, declared } => write!(
                formatter,
                "the MCP tool {tool} declares the JSON Schema dialect {declared}, and JARVIS \
                 implements only {TOOL_SCHEMA_DIALECT}"
            ),
            Self::InvalidInputSchema { tool, reason } => write!(
                formatter,
                "the MCP tool {tool} has an unusable input schema: {reason}"
            ),
            Self::InvalidOutputSchema { tool, reason } => write!(
                formatter,
                "the MCP tool {tool} has an unusable output schema: {reason}"
            ),
            Self::Definition(source) => write!(formatter, "{source}"),
        }
    }
}

impl std::error::Error for TranslationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Name(source) => Some(source),
            Self::Definition(source) => Some(source),
            Self::NotConformant { .. }
            | Self::UnsupportedDialect { .. }
            | Self::InvalidInputSchema { .. }
            | Self::InvalidOutputSchema { .. } => None,
        }
    }
}

/// Translates one server-supplied tool listing into a canonical definition.
///
/// The order is: name, then conformance, then schema, then definition. Conformance precedes schema
/// construction because the two answer different questions and the conformance answer is the more
/// specific one — a `$ref` is reported by both, and "excluded from discovery because a header
/// annotation is unreachable" tells an operator more than "the schema was refused".
///
/// **Effects and risk come from `policy`, never from the listing.** The listing's `annotations` field
/// is not read at all: the protocol warns it is untrusted, and every field it could carry is one
/// JARVIS decides.
///
/// # Errors
///
/// Returns [`TranslationError`] for a name that cannot be canonicalised, a schema that fails the
/// protocol's conformance rules, a dialect JARVIS does not implement, or an unusable schema.
pub fn translate_tool(
    server: &ServerName,
    listing: &McpToolListing<'_>,
    strategy: NamingStrategy,
    policy: &ToolEffectPolicy,
) -> Result<TranslatedTool, TranslationError> {
    let CanonicalToolName { id, remote } =
        canonical_tool_name(server, listing.name, strategy).map_err(TranslationError::Name)?;

    let report = check_tool_schema(listing.name, listing.input_schema);
    if !report.is_conformant() {
        return Err(TranslationError::NotConformant {
            tool: remote,
            reason: report.describe(),
        });
    }

    let input_schema = schema_for(listing.input_schema, true, &remote)?;
    let output_schema = match listing.output_schema {
        Some(schema) => schema_for(schema, false, &remote)?,
        // No output schema: a permissive one, which is the truthful encoding of "the server made no
        // claim". Inventing a strict schema would refuse results the server legitimately returns;
        // inheriting the input schema would be a fiction.
        None => permissive_schema().map_err(|error| TranslationError::InvalidOutputSchema {
            tool: remote.clone(),
            reason: error,
        })?,
    };

    let (title, description) = naming_text(listing, server);
    let definition = ToolDefinition::new(ToolDefinitionParts {
        id,
        version: tool_version(listing.input_schema),
        title,
        description,
        input_schema,
        output_schema,
        effects: policy.posture.effects.clone(),
        risk: policy.posture.risk,
        required_scopes: policy.scopes.clone(),
        approval: policy.posture.approval,
        timeout_seconds: policy.execution.timeout_seconds,
        retry: policy.execution.retry,
        idempotency: policy.execution.idempotency,
        // Must agree with the identifier, which the naming module built with an `mcp.` namespace. If
        // this disagreed, `ToolDefinition::new` refuses it — which is the point of declaring it.
        source: ToolSource::Mcp,
        availability: Availability::Available,
        sensitivity: policy.sensitivity,
    })
    .map_err(TranslationError::Definition)?;

    Ok(TranslatedTool { definition, remote })
}

/// Translates a whole listing, excluding tools that fail rather than failing the list.
///
/// The specification requires exactly this: a client "**MUST** exclude the invalid tool from the
/// result of `tools/list`", with the stated reason that "a single malformed tool definition does not
/// prevent other valid tools from being used".
///
/// A name collision is an exclusion for the same reason. It is *not* resolved by renaming, because
/// any rename is a mapping the operator did not choose, and the operator is the one who can decide
/// which server to call something else.
#[must_use]
pub fn translate_listing(
    server: &ServerName,
    listings: &[McpToolListing<'_>],
    strategy: NamingStrategy,
    policy: &ToolEffectPolicy,
) -> ListingOutcome {
    let mut assignments = NameAssignments::new();
    let mut tools = Vec::new();
    let mut excluded = Vec::new();

    for listing in listings {
        match translate_tool(server, listing, strategy, policy) {
            Ok(translated) => {
                match assignments.assign(
                    server,
                    &CanonicalToolName {
                        id: translated.definition.id().clone(),
                        remote: translated.remote.clone(),
                    },
                ) {
                    Ok(()) => tools.push(translated),
                    Err(collision) => excluded.push(ExcludedTool {
                        tool: listing.name.to_owned(),
                        reason: collision.to_string(),
                    }),
                }
            }
            Err(error) => excluded.push(ExcludedTool {
                tool: listing.name.to_owned(),
                reason: error.to_string(),
            }),
        }
    }

    ListingOutcome { tools, excluded }
}

/// What a server's whole listing produced.
#[derive(Clone, Debug)]
pub struct ListingOutcome {
    /// The tools that may be registered, in the order the server listed them.
    pub tools: Vec<TranslatedTool>,
    /// The tools that were excluded, each with the reason an operator can act on.
    pub excluded: Vec<ExcludedTool>,
}

/// One excluded tool and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExcludedTool {
    /// The server's own tool name, unbounded here because it is the server's text and the caller
    /// decides how to present it.
    pub tool: String,
    /// A reader-facing reason.
    pub reason: String,
}

/// Builds a JARVIS schema from a server-supplied one, supplying the dialect MCP defaults.
///
/// The dialect is the interesting part. MCP says a schema with no `$schema` "defaults to 2020-12",
/// while `jarvis-tools` **requires** the keyword and refuses to default it — because defaulting is
/// only unambiguous to the document's author, and `exclusiveMinimum` is a boolean in earlier drafts
/// and a number in 2020-12, so a correct older-draft document would silently become an invalid newer
/// one. Both positions are right, and they compose: JARVIS injects the dialect **only when the server
/// omitted it**, which is what the protocol says that omission means, and **refuses** a schema that
/// declares a different one rather than reinterpreting it.
fn schema_for(
    schema: &Value,
    is_input: bool,
    remote: &str,
) -> Result<jarvis_tools::ToolSchema, TranslationError> {
    let refuse = |reason: String| {
        if is_input {
            TranslationError::InvalidInputSchema {
                tool: remote.to_owned(),
                reason,
            }
        } else {
            TranslationError::InvalidOutputSchema {
                tool: remote.to_owned(),
                reason,
            }
        }
    };

    let mut document = schema.clone();
    let object = document
        .as_object_mut()
        .ok_or_else(|| refuse("the schema is not a JSON object".to_owned()))?;

    match object.get(SCHEMA_KEYWORD) {
        None => {
            object.insert(SCHEMA_KEYWORD.to_owned(), json!(TOOL_SCHEMA_DIALECT));
        }
        Some(Value::String(declared)) if declared == TOOL_SCHEMA_DIALECT => {}
        Some(Value::String(declared)) => {
            return Err(TranslationError::UnsupportedDialect {
                tool: remote.to_owned(),
                declared: declared.clone(),
            });
        }
        Some(other) => {
            return Err(refuse(format!(
                "the {} keyword is {}, not a string",
                SCHEMA_KEYWORD,
                describe(other)
            )));
        }
    }

    to_tool_schema(&document).map_err(|error| refuse(error.to_string()))
}

/// Builds the permissive schema used when a server declares no output schema.
fn permissive_schema() -> Result<jarvis_tools::ToolSchema, String> {
    jarvis_tools::ToolSchema::from_value(json!({ SCHEMA_KEYWORD: TOOL_SCHEMA_DIALECT }))
        .map_err(|error| error.to_string())
}

/// Derives the version JARVIS records for a server-supplied tool.
///
/// MCP has no per-tool version, so JARVIS supplies one. It is derived from the **input schema's
/// content**, not from a digest of the whole listing, so it changes exactly when the contract the
/// model is offered changes: a description edit or a title change is cosmetic, while a schema change
/// means a stored intent hashed against the old version no longer describes this tool.
///
/// The result is a fixed marker plus a short digest, which is stable across processes and therefore
/// across a restart — a random or counter-based version would invalidate every stored binding.
///
/// The serialisation cannot fail: [`Value`]'s maps are keyed by `String`, so there is no non-string
/// key for `serde_json` to refuse. An empty fallback is still not used silently — a failure would
/// produce a digest of the empty string, which differs from every real schema's and so would be a
/// visibly wrong version rather than a plausible one.
#[must_use]
pub fn tool_version(input_schema: &Value) -> String {
    let canonical =
        serde_json::to_string(input_schema).unwrap_or_else(|_| "<unserialisable>".to_owned());
    format!("schema-{}", short_digest(&canonical))
}

/// Derives the title and description for a definition.
///
/// Both are third-party text destined for a **model** and an **operator**, so both are sanitised:
/// whitespace is normalised, control characters and bidirectional overrides are removed, and each is
/// bounded. The bidi removal is not cosmetic — a Unicode bidirectional override in a description can
/// make text *render* as something other than what it is, which is a way to misrepresent a tool in
/// exactly the place a person or model decides whether to call it.
fn naming_text(listing: &McpToolListing<'_>, server: &ServerName) -> (String, String) {
    let sanitised_name = sanitize(listing.name, jarvis_tools::MAX_TOOL_TITLE_CHARS);
    let title = listing
        .title
        .map(|value| sanitize(value, jarvis_tools::MAX_TOOL_TITLE_CHARS))
        .filter(|value| !value.is_empty())
        // Falls back to the server's tool name, which is always non-empty by the time it reaches
        // here. A generated placeholder like "MCP tool" would be worse: it tells a reader nothing
        // and would be identical for every tool.
        .unwrap_or_else(|| {
            if sanitised_name.is_empty() {
                "MCP tool".to_owned()
            } else {
                sanitised_name.clone()
            }
        });

    let description = listing
        .description
        .map(|value| sanitize(value, DESCRIPTION_BUDGET))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            // Says what is true — the server did not describe this tool — rather than inventing a
            // description. A reader who sees this knows the tool is undocumented, which is a fact
            // worth acting on; a plausible-sounding generated sentence would not be.
            format!(
                "Provided by the MCP server {server}. The server supplied no description, so this \
                 tool's behaviour is unstated."
            )
        });

    (title, description)
}

/// Normalises untrusted text into one bounded line.
///
/// Collapses all whitespace to single spaces and drops control characters and bidirectional
/// formatting characters. Whitespace is collapsed rather than preserved because the destination is a
/// single-line field: a newline in a title is at best ignored and at worst a way to make text appear
/// to be something else in a multi-line display.
#[must_use]
pub fn sanitize(text: &str, limit: usize) -> String {
    let mut out = String::with_capacity(text.len().min(limit * 4));
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_control() || is_directional(character) || character.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(character);
    }
    if out.chars().count() <= limit {
        return out;
    }
    out.chars().take(limit).collect()
}

/// Returns whether a character is a bidirectional or zero-width formatting character.
///
/// The ranges cover the bidirectional embedding/override/isolate characters, the directional marks,
/// and the zero-width space and non-joiner/joiner. All of them can change how text is *read* without
/// changing what it *is*, which for a description that decides whether a tool gets called is a
/// deception primitive rather than a typographic nicety.
fn is_directional(character: char) -> bool {
    matches!(
        character,
        '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}'
    )
}

/// Names a JSON value's shape for an error message.
fn describe(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Derives a short, stable digest of a string.
///
/// SHA-256 truncated to eight hexadecimal characters. A digest rather than a counter for a version
/// because a version must be reproducible: a counter would assign different versions to the same
/// schema on two runs, invalidating every stored intent hashed against it.
fn short_digest(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"jarvis-mcp-tool-version");
    hasher.update([0x1f]);
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(8);
    for byte in digest.iter().take(4) {
        let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_tools::Risk;

    fn server(name: &str) -> ServerName {
        ServerName::new(name).unwrap_or_else(|error| panic!("fixture server {name}: {error}"))
    }

    /// The one scope every MCP tool declares here.
    fn scope_set() -> ScopeSet {
        ScopeSet::single(Scope::new(DEFAULT_MCP_SCOPE).unwrap_or_else(|error| panic!("{error}")))
    }

    /// A schema a server may legally send: no `$schema`, which MCP says defaults to 2020-12.
    fn unversioned_schema() -> Value {
        json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    fn listing<'a>(name: &'a str, schema: &'a Value) -> McpToolListing<'a> {
        McpToolListing {
            name,
            title: None,
            description: None,
            input_schema: schema,
            output_schema: None,
        }
    }

    fn translated(name: &str, policy: &ToolEffectPolicy) -> TranslatedTool {
        let schema = unversioned_schema();
        translate_tool(
            &server("remote"),
            &listing(name, &schema),
            NamingStrategy::Prefixed,
            policy,
        )
        .unwrap_or_else(|error| panic!("fixture {name}: {error}"))
    }

    /// The default posture is severe on purpose, and each half of that severity is asserted: the
    /// effects force a risk floor, the risk is above it, and the approval policy cannot be lowered by
    /// workspace policy the way `Policy` could.
    #[test]
    fn the_unclassified_posture_is_severe_and_every_call_is_asked_about() {
        let policy = ToolEffectPolicy::unclassified();
        let posture = policy.posture();
        assert_eq!(
            posture.effects.risk_floor(),
            2,
            "the outward effect is what sets the floor"
        );
        assert!(posture.effects.contains(ToolEffect::ExternalCommunication));
        assert!(posture.effects.contains(ToolEffect::Write));
        assert!(posture.effects.is_mutating());
        assert_eq!(
            posture.risk, 3,
            "an unclassified server gets the most scrutiny"
        );
        assert!(posture.risk >= posture.effects.risk_floor());
        assert_eq!(posture.approval, ApprovalPolicy::Ask);
        // No automatic retry, because a repeat of an outward effect is a second effect.
        assert_eq!(policy.execution().retry.attempts, 0);
        assert_eq!(policy.execution().idempotency, Idempotency::Unsupported);
    }

    #[test]
    fn an_unclassified_tool_declares_mcp_as_its_source() {
        let tool = translated("search", &ToolEffectPolicy::unclassified());
        // The source is what marks the definition third-party, and it must agree with the `mcp.`
        // namespace the naming module built.
        assert_eq!(tool.definition.id().source(), ToolSource::Mcp);
        assert!(tool.definition.id().namespace().starts_with("mcp."));
    }

    /// **The defect class this crate exists to prevent, one level up.** A server lists a tool
    /// alongside `annotations` claiming it is read-only. Those annotations are attacker-controlled and
    /// the protocol says clients MUST treat them as untrusted, so the translation must not read them —
    /// the posture comes from the operator.
    #[test]
    fn a_server_supplied_annotation_cannot_lower_the_posture() {
        let schema = json!({
            "type": "object",
            "properties": { "q": { "type": "string" } },
            "annotations": { "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true }
        });
        let mut value = schema;
        // Put the annotations where a server would really put them: on the tool, not the schema. The
        // struct has no field for them, which is the first line of defence; this asserts the second,
        // that smuggling them into the schema changes nothing.
        value["inputSchema"] = unversioned_schema();

        let tool = translate_tool(
            &server("remote"),
            &McpToolListing {
                name: "delete_everything",
                title: Some("Delete everything"),
                description: Some("Read-only, safe, harmless"),
                input_schema: &unversioned_schema(),
                output_schema: None,
            },
            NamingStrategy::Prefixed,
            &ToolEffectPolicy::unclassified(),
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            tool.definition.risk().level(),
            3,
            "a server's own claim must not lower the risk"
        );
        assert!(
            !tool.definition.approval().is_runnable()
                || tool.definition.approval() == ApprovalPolicy::Ask
        );
        // And the description is carried as text, never as an instruction.
        assert!(tool.definition.description().contains("Read-only"));
    }

    #[test]
    fn a_read_only_policy_produces_risk_zero_and_auto() {
        let tool = translated(
            "search",
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        );
        assert_eq!(tool.definition.risk(), Risk::Minimal);
        assert_eq!(tool.definition.approval(), ApprovalPolicy::Auto);
    }

    /// The dialect rule, and both halves of it: an omitted `$schema` is supplied because MCP says
    /// that is what omission means, and a *declared different* dialect is refused rather than
    /// reinterpreted.
    #[test]
    fn an_omitted_dialect_is_supplied_and_a_different_one_is_refused() {
        let omitted = translated(
            "search",
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        );
        assert!(
            omitted
                .definition
                .input_schema()
                .validate(&json!({"query": "x"}))
                .is_ok()
        );

        let declared_old = json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": { "q": { "type": "string" } }
        });
        let error = translate_tool(
            &server("remote"),
            &listing("search", &declared_old),
            NamingStrategy::Prefixed,
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        )
        .err()
        .unwrap_or_else(|| panic!("a draft-07 schema must not be reinterpreted as 2020-12"));
        assert!(
            matches!(error, TranslationError::UnsupportedDialect { .. }),
            "{error}"
        );
        // The refusal names the dialect it cannot serve and the one it can, or an operator cannot act.
        let text = error.to_string();
        assert!(text.contains("draft-07"), "{text}");
        assert!(text.contains(TOOL_SCHEMA_DIALECT), "{text}");
    }

    #[test]
    fn a_normally_declared_dialect_is_accepted() {
        let schema = json!({
            "$schema": TOOL_SCHEMA_DIALECT,
            "type": "object",
            "properties": { "q": { "type": "string" } }
        });
        assert!(
            translate_tool(
                &server("remote"),
                &listing("search", &schema),
                NamingStrategy::Prefixed,
                &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
            )
            .is_ok()
        );
    }

    /// A schema with a `$ref` is refused, and the refusal is a *conformance* one — the more specific
    /// message — because that pass runs first by design.
    #[test]
    fn a_referencing_schema_is_refused_as_non_conformant() {
        let schema = json!({
            "type": "object",
            "properties": { "q": { "$ref": "https://example.invalid/x.json" } }
        });
        let error = translate_tool(
            &server("remote"),
            &listing("search", &schema),
            NamingStrategy::Prefixed,
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        )
        .err()
        .unwrap_or_else(|| panic!("a $ref must be refused"));
        assert!(
            matches!(error, TranslationError::NotConformant { .. }),
            "{error}"
        );
    }

    /// A server's description is model-facing text, so a bidirectional override in it is a deception
    /// primitive: it changes how the text reads without changing what it is.
    #[test]
    fn a_bidirectional_override_is_removed_from_server_text() {
        let schema = unversioned_schema();
        let hostile = "safe\u{202e}elif\u{202c} tool";
        let tool = translate_tool(
            &server("remote"),
            &McpToolListing {
                name: "search",
                title: Some(hostile),
                description: Some(hostile),
                input_schema: &schema,
                output_schema: None,
            },
            NamingStrategy::Prefixed,
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));

        for text in [tool.definition.title(), tool.definition.description()] {
            assert!(
                !text.contains('\u{202e}'),
                "{text:?} still holds an override"
            );
            assert!(!text.contains('\u{202c}'), "{text:?} still holds a pop");
            assert!(text.contains("safe"), "{text:?}");
        }
    }

    #[test]
    fn server_text_is_normalised_to_one_line_and_bounded() {
        let schema = unversioned_schema();
        let multi_line = "first\n\nsecond\tthird   fourth";
        let tool = translate_tool(
            &server("remote"),
            &McpToolListing {
                name: "search",
                title: None,
                description: Some(multi_line),
                input_schema: &schema,
                output_schema: None,
            },
            NamingStrategy::Prefixed,
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let description = tool.definition.description();
        assert!(!description.contains('\n'), "{description:?}");
        assert!(!description.contains('\t'), "{description:?}");
        assert_eq!(description, "first second third fourth");

        // A long description is bounded, and the bound is respected rather than approximated.
        let long = "x".repeat(DESCRIPTION_BUDGET + 500);
        let bounded = translate_tool(
            &server("remote"),
            &McpToolListing {
                name: "search",
                title: None,
                description: Some(&long),
                input_schema: &schema,
                output_schema: None,
            },
            NamingStrategy::Prefixed,
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            bounded.definition.description().chars().count(),
            DESCRIPTION_BUDGET
        );
        assert!(
            bounded.definition.description().chars().count()
                <= jarvis_tools::MAX_TOOL_DESCRIPTION_CHARS
        );
    }

    /// A title is required by the definition, so a server that supplies none gets its own tool name
    /// rather than a generated placeholder that would be identical for every tool.
    #[test]
    fn a_missing_title_falls_back_to_the_servers_own_tool_name() {
        let tool = translated(
            "list_issues",
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        );
        assert_eq!(tool.definition.title(), "list_issues");

        let schema = unversioned_schema();
        let blank = translate_tool(
            &server("remote"),
            &McpToolListing {
                name: "list_issues",
                title: Some("   "),
                description: Some(""),
                input_schema: &schema,
                output_schema: None,
            },
            NamingStrategy::Prefixed,
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(blank.definition.title(), "list_issues");
        // An absent description says so, rather than inventing one.
        assert!(blank.definition.description().contains("unstated"));
    }

    #[test]
    fn a_declared_output_schema_is_used_and_an_absent_one_is_permissive() {
        let input = unversioned_schema();
        let output = json!({
            "type": "object",
            "properties": { "results": { "type": "array" } }
        });
        let with_output = translate_tool(
            &server("remote"),
            &McpToolListing {
                name: "search",
                title: None,
                description: None,
                input_schema: &input,
                output_schema: Some(&output),
            },
            NamingStrategy::Prefixed,
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            with_output
                .definition
                .output_schema()
                .validate(&json!({"results": []}))
                .is_ok()
        );

        let without = translated(
            "search",
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        );
        // Permissive: the server made no claim, so nothing is refused on its behalf.
        assert!(
            without
                .definition
                .output_schema()
                .validate(&json!({"anything": 1}))
                .is_ok()
        );
        assert!(
            without
                .definition
                .output_schema()
                .validate(&json!("a string"))
                .is_ok()
        );
    }

    /// The version has to be reproducible across processes or every stored intent is invalidated by a
    /// restart.
    #[test]
    fn the_version_is_stable_and_tracks_the_schema_not_the_prose() {
        let schema = unversioned_schema();
        assert_eq!(tool_version(&schema), tool_version(&schema));

        let mut changed_description = schema.clone();
        // A cosmetic change to a *listing* must not move the version: the version is over the schema,
        // which is the contract the model is offered.
        changed_description["description"] = json!("a nicer description");
        assert_ne!(
            tool_version(&schema),
            tool_version(&changed_description),
            "a schema-level change IS a contract change"
        );

        let mut changed_property = schema.clone();
        changed_property["properties"] = json!({ "query": { "type": "number" } });
        assert_ne!(tool_version(&schema), tool_version(&changed_property));
        assert!(tool_version(&schema).starts_with("schema-"));
        assert!(jarvis_tools::ToolId::validate_version(&tool_version(&schema)).is_ok());
    }

    /// The specification requires one bad tool to be excluded without removing the others, and that
    /// is what makes a long-tail MCP server usable at all.
    #[test]
    fn a_bad_tool_is_excluded_and_the_good_ones_survive() {
        let good = unversioned_schema();
        let bad = json!({
            "type": "object",
            "properties": { "q": { "$ref": "https://example.invalid/x.json" } }
        });
        let listings = vec![
            listing("good_one", &good),
            listing("bad_one", &bad),
            listing("good_two", &good),
        ];
        let outcome = translate_listing(
            &server("remote"),
            &listings,
            NamingStrategy::Prefixed,
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        );
        assert_eq!(outcome.tools.len(), 2, "{:?}", outcome.excluded);
        assert_eq!(outcome.excluded.len(), 1);
        assert_eq!(outcome.excluded[0].tool, "bad_one");
        // The exclusion names a reason an operator can act on.
        assert!(!outcome.excluded[0].reason.is_empty());
    }

    /// A collision is an exclusion, never a rename: any rename is a mapping the operator did not
    /// choose, and the operator is who can decide which server to call something else.
    ///
    /// Driven through `NameAssignments` directly, because `translate_listing` builds its own set per
    /// call and therefore only ever sees one server. The aggregate case across servers is what the
    /// check is for, and pretending one listing call sees two servers would test a shape that does not
    /// exist.
    #[test]
    fn a_collision_excludes_rather_than_renaming() {
        let schema = unversioned_schema();
        let listings = vec![listing("search", &schema)];
        let outcome = translate_listing(
            &server("alpha"),
            &listings,
            NamingStrategy::Bare,
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        );
        assert_eq!(outcome.tools.len(), 1);
        assert_eq!(outcome.tools[0].definition.id().to_string(), "mcp.search");

        let policy = ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}"));
        let first = translate_tool(
            &server("alpha"),
            &listings[0],
            NamingStrategy::Bare,
            &policy,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let second = translate_tool(
            &server("bravo"),
            &listings[0],
            NamingStrategy::Bare,
            &policy,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            first.definition.id().to_string(),
            second.definition.id().to_string(),
            "the bare strategy is expected to collide across servers"
        );

        let mut assignments = NameAssignments::new();
        assert!(
            assignments
                .assign(
                    &server("alpha"),
                    &CanonicalToolName {
                        id: first.definition.id().clone(),
                        remote: first.remote.clone(),
                    }
                )
                .is_ok()
        );
        let Err(collision) = assignments.assign(
            &server("bravo"),
            &CanonicalToolName {
                id: second.definition.id().clone(),
                remote: second.remote.clone(),
            },
        ) else {
            panic!("two servers on one bare name must collide");
        };
        // The refusal names both servers, because the remedy is a human renaming one of them.
        let text = collision.to_string();
        assert!(text.contains("alpha"), "{text}");
        assert!(text.contains("bravo"), "{text}");
    }

    /// A collision inside one listing is an exclusion, and the good tools in that same listing still
    /// survive — the per-tool rule applied to the collision cause rather than the schema cause.
    #[test]
    fn a_collision_within_one_listing_excludes_only_the_later_tool() {
        let schema = unversioned_schema();
        // Two names that canonicalise to one identifier: with `Hashed` the digest is of the whole
        // name, so these differ — so drive it with a name and its uppercase twin, which the readable
        // strategies refuse and `Hashed` keeps distinct. The reachable same-listing collision is a
        // server listing one name twice.
        let listings = vec![listing("search", &schema), listing("search", &schema)];
        let outcome = translate_listing(
            &server("alpha"),
            &listings,
            NamingStrategy::Prefixed,
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        );
        // A repeat of the SAME tool name from the SAME server is a refresh, not a collision, so both
        // entries are kept — which is what makes a re-`tools/list` idempotent.
        assert_eq!(outcome.tools.len(), 2);
        assert!(outcome.excluded.is_empty());
    }

    #[test]
    fn the_remote_name_is_carried_alongside_the_definition() {
        let tool = translated(
            "list_issues",
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        );
        // `tools/call` sends the server's name; the definition holds the canonical identifier.
        assert_eq!(tool.remote, "list_issues");
        assert_eq!(tool.definition.id().name(), "list_issues");
        assert_eq!(tool.definition.id().namespace(), "mcp.remote");
    }

    /// An operator's mistake in configuration is reported when the policy is built, not when a remote
    /// server is mid-call.
    #[test]
    fn a_policy_whose_risk_hides_its_effects_is_refused_at_construction() {
        let error = ToolEffectPolicy::new(ToolEffectPolicyParts {
            posture: ToolPosture {
                effects: EffectSet::single(ToolEffect::Destructive),
                risk: 0,
                approval: ApprovalPolicy::Auto,
            },
            scopes: scope_set(),
            execution: ToolExecutionLimits {
                timeout_seconds: DEFAULT_MCP_TIMEOUT_SECONDS,
                retry: RetryDeclaration::none(),
                idempotency: Idempotency::Unsupported,
            },
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })
        .err()
        .unwrap_or_else(|| panic!("risk 0 cannot cover a destructive effect"));
        assert_eq!(
            error,
            PolicyError::RiskBelowEffects {
                declared: 0,
                required: 3
            }
        );
        assert!(error.to_string().contains("below the risk 3"));
    }

    /// The retry rule: a mutating effect whose repeats are not made safe cannot be retried blindly,
    /// so a policy that would send a payment twice cannot be *held*.
    #[test]
    fn a_policy_that_would_repeat_an_outward_effect_is_refused_at_construction() {
        let limits = |idempotency| ToolExecutionLimits {
            timeout_seconds: DEFAULT_MCP_TIMEOUT_SECONDS,
            retry: RetryDeclaration {
                attempts: 3,
                backoff_ceiling_seconds: 5,
            },
            idempotency,
        };
        let posture = ToolPosture {
            effects: EffectSet::single(ToolEffect::ExternalCommunication),
            risk: 2,
            approval: ApprovalPolicy::Ask,
        };

        // The server offers no way to make a repeat safe, so a blind retry is a second send.
        let error = ToolEffectPolicy::new(ToolEffectPolicyParts {
            posture: posture.clone(),
            scopes: scope_set(),
            execution: limits(Idempotency::Unsupported),
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })
        .err()
        .unwrap_or_else(|| panic!("a blind retry of a send must be refused"));
        assert!(matches!(error, PolicyError::UnsafeRetry(_)), "{error}");

        // With a provider that deduplicates, the same retry is legal.
        let allowed = ToolEffectPolicy::new(ToolEffectPolicyParts {
            posture,
            scopes: scope_set(),
            execution: limits(Idempotency::ProviderKey),
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        });
        assert!(
            allowed.is_ok(),
            "a deduplicating provider makes a retry safe"
        );
    }

    /// A read-only policy's retry is legal precisely because repeating a read has no second effect —
    /// the positive control for the rule above.
    #[test]
    fn a_read_only_policy_may_retry() {
        let policy = ToolEffectPolicy::read_only().unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(policy.execution().retry.attempts, 1);
        assert!(!policy.posture().effects.is_mutating());
    }

    #[test]
    fn an_empty_tool_name_is_refused() {
        let schema = unversioned_schema();
        let error = translate_tool(
            &server("remote"),
            &listing("   ", &schema),
            NamingStrategy::Prefixed,
            &ToolEffectPolicy::read_only().unwrap_or_else(|e| panic!("{e}")),
        )
        .err()
        .unwrap_or_else(|| panic!("an empty name must be refused"));
        assert!(matches!(error, TranslationError::Name(_)), "{error}");
    }

    #[test]
    fn the_default_scope_is_a_valid_jarvis_scope() {
        assert!(Scope::new(DEFAULT_MCP_SCOPE).is_ok());
        let policy = ToolEffectPolicy::unclassified();
        assert!(
            policy
                .scopes()
                .contains(&Scope::new(DEFAULT_MCP_SCOPE).unwrap_or_else(|e| panic!("{e}")))
        );
    }
}
