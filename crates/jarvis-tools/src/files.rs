//! The read-only filesystem tool: the first real [`ToolExecutor`] adapter.
//!
//! # What this slice is, and what it is not
//!
//! `P3-006` is the first adapter, and being first is most of its value: `P3-001` through `P3-005`
//! built a contract, a registry, a policy engine, a durable approval, and a lifecycle, and until this
//! slice nothing ran any of them. This module is the consumer that shows whether those pieces fit.
//!
//! It is **read-only**, and the reason to implement the read half first is that a read cannot be the
//! mistake that destroys something: the failure mode of a bug here is disclosure, which the
//! confinement in [`crate::WorkspaceRoots`] exists to prevent, rather than destruction, which would
//! need an undo design this slice does not have.
//!
//! # The two tools
//!
//! | Tool | Effect | What it does |
//! | --- | --- | --- |
//! | `jarvis.files.read` | `read_only` | returns one file's contents, bounded |
//! | `jarvis.files.list` | `read_only` | returns one directory's entry names, sorted |
//!
//! Both are risk 0, `Auto` approval, and idempotency `Required`, which is what the guidance table in
//! `docs/architecture/tools-and-connectors.md` calls for when reading granted content with a scope:
//! no human decides, and repeating the read is meaningful so a retry is safe.
//!
//! # Why the bound is applied at the read and not only by the type
//!
//! [`MAX_TOOL_OUTPUT_BYTES`] is 32 KiB, and a 4 GB file is a plausible thing for a model to name. So
//! the file is read through a **limited reader** rather than read into memory and then bounded:
//! [`BoundedOutput`] bounds the result *after* the allocation, which would turn a large read into an
//! out-of-memory rather than a truncated answer. The bound is applied twice and both applications
//! matter, but the read is the one that matters for a large file.

use std::io::Read;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use jarvis_core::{MAX_OUTCOME_DETAIL_CHARS, Sensitivity, SystemClock, UtcTimestamp};
use serde_json::{Value, json};

use crate::ToolOutcomeRecord;
use crate::definition::{ToolDefinition, ToolDefinitionError, ToolDefinitionParts};
use crate::effect::{EffectSet, ToolEffect};
use crate::execution::{BoundedOutput, MAX_TOOL_OUTPUT_BYTES, ProviderEvidence, ToolCallResult};
use crate::executor::{AdapterError, ToolExecutionRequest, ToolExecutor};
use crate::identifier::ToolId;
use crate::policy::{
    ApprovalPolicy, Availability, Idempotency, RetryDeclaration, ToolSensitivity, ToolSource,
};
use crate::schema::{SchemaError, TOOL_SCHEMA_DIALECT, ToolSchema};
use crate::scope::{Scope, ScopeSet};
use crate::workspace::WorkspaceRoots;

/// The tool that reads one file.
pub const READ_TOOL: &str = "jarvis.files.read";

/// The tool that lists one directory.
pub const LIST_TOOL: &str = "jarvis.files.list";

/// The tool that creates a file or appends to one. It can never replace or remove existing content.
pub const WRITE_TOOL: &str = "jarvis.files.write";

/// The tool that replaces one exact piece of text in an existing file.
pub const EDIT_TOOL: &str = "jarvis.files.edit";

/// The most characters one write or one replacement may carry.
///
/// A held call keeps its arguments so the owner can see them (`ADR-0130`), and a pending approval holds at most
/// 8 KiB. Keeping a call under that keeps it **decidable**; a larger file is written in several appends.
pub const MAX_WRITE_CHARS: usize = 4000;

/// The most characters of text to find in an edit.
pub const MAX_EDIT_FIND_CHARS: usize = 2000;

/// The largest file an edit will read and rewrite. Larger is refused, never truncated.
const MAX_EDITABLE_FILE_BYTES: u64 = 512 * 1024;

/// Maximum characters in a path argument.
///
/// Not a security control — confinement is — but a bound on how much work one call can ask for. 4096
/// is the classic path ceiling and comfortably above what a workspace path needs.
pub const MAX_PATH_ARGUMENT_CHARS: usize = 4096;

/// Maximum entries a single listing returns.
///
/// A listing is not bounded by [`MAX_TOOL_OUTPUT_BYTES`] until it is serialized, so a directory with a
/// million entries would build a million-element vector and then discard most of it. The count is
/// capped at the source and the result says it was capped.
pub const MAX_LISTED_ENTRIES: usize = 1000;

/// How long a filesystem call may take, in seconds.
///
/// Short deliberately. A local read has no network to wait on, so this bounds a read from a slow or
/// disconnected network mount rather than a normal file.
const TIMEOUT_SECONDS: u32 = 30;

/// The input schema of `jarvis.files.read`.
const READ_INPUT_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["path"],
  "properties": {
    "path": {
      "type": "string",
      "minLength": 1,
      "maxLength": 4096,
      "description": "A path relative to a granted workspace root."
    }
  }
}"#;

/// The output schema of `jarvis.files.read`.
const READ_OUTPUT_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["content", "truncated"],
  "properties": {
    "content": { "type": "string" },
    "truncated": {
      "type": "boolean",
      "description": "True when the content was cut at the output bound."
    }
  }
}"#;

/// The input schema of `jarvis.files.list`.
const LIST_INPUT_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["path"],
  "properties": {
    "path": {
      "type": "string",
      "minLength": 1,
      "maxLength": 4096,
      "description": "A directory path relative to a granted workspace root."
    }
  }
}"#;

/// The output schema of `jarvis.files.list`.
const LIST_OUTPUT_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["entries", "truncated"],
  "properties": {
    "entries": { "type": "array", "items": { "type": "string" } },
    "truncated": {
      "type": "boolean",
      "description": "True when the listing was cut at the entry bound."
    }
  }
}"#;

/// The input schema of `jarvis.files.write`.
const WRITE_INPUT_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["path", "content"],
  "properties": {
    "path": { "type": "string", "minLength": 1, "maxLength": 4096, "description": "A file path relative to a granted workspace root. Missing folders are created." },
    "content": { "type": "string", "maxLength": 4000, "description": "The text to write. Write a longer file in several appends." },
    "mode": { "type": "string", "enum": ["create", "append"], "description": "create (default) makes a NEW file and refuses if it exists; append adds to the end of a file, creating it if missing." }
  }
}"#;

/// The input schema of `jarvis.files.edit`.
const EDIT_INPUT_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["path", "find", "replace"],
  "properties": {
    "path": { "type": "string", "minLength": 1, "maxLength": 4096, "description": "An existing file path relative to a granted workspace root." },
    "find": { "type": "string", "minLength": 1, "maxLength": 2000, "description": "The exact text to replace. It must occur exactly once; include surrounding lines to make it unique." },
    "replace": { "type": "string", "maxLength": 4000, "description": "The text to put in its place (may be empty to delete the text)." }
  }
}"#;

/// The output schema of the two write tools.
const WRITE_OUTPUT_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false,
  "required": ["outcome"],
  "properties": {
    "outcome": { "type": "string", "enum": ["created", "appended", "edited"] },
    "bytes": { "type": "integer", "description": "Bytes written or added." }
  }
}"#;
/// Reads files beneath granted workspace roots.
///
/// Holds [`WorkspaceRoots`], which is the confinement boundary. An instance can only be built from
/// roots that opened successfully, so "this adapter has a boundary" is a property of the type rather
/// than of its configuration.
pub struct FilesystemTool {
    roots: WorkspaceRoots,
}

impl FilesystemTool {
    /// Builds the adapter over the granted roots.
    ///
    /// Takes ownership rather than a reference: the handle set *is* the boundary, and a borrowed one
    /// would let a caller replace or drop it while a call is in flight.
    #[must_use]
    pub const fn new(roots: WorkspaceRoots) -> Self {
        Self { roots }
    }

    /// Returns the roots this adapter is confined to, for diagnostics.
    #[must_use]
    pub const fn roots(&self) -> &WorkspaceRoots {
        &self.roots
    }

    /// Builds the canonical definitions of the four tools: read, list, write and edit.
    ///
    /// # Errors
    ///
    /// Returns [`FilesystemToolError`] when a fixed constant is rejected. Every definition is a constant, so a
    /// failure is an authoring error rather than a runtime condition — the test module asserts all are accepted,
    /// which is what keeps the schema strings and the declarations honest.
    pub fn definitions() -> Result<Vec<ToolDefinition>, FilesystemToolError> {
        Ok(vec![
            Self::definition(READ_TOOL)?,
            Self::definition(LIST_TOOL)?,
            Self::definition(WRITE_TOOL)?,
            Self::definition(EDIT_TOOL)?,
        ])
    }

    /// Builds the canonical definition of one of this adapter's tools.
    ///
    /// # Errors
    ///
    /// Returns [`FilesystemToolError::UnknownTool`] for a name this adapter does not provide, and
    /// [`FilesystemToolError`] for a rejected constant.
    ///
    /// An unknown name is an error rather than defaulting to `read`: a fallback would make a mistyped
    /// tool name silently read a file.
    ///
    /// # What each tool declares, and why
    ///
    /// - `read` and `list`: `read_only`, risk 0, runs when scoped.
    /// - `write`: `write`, risk 1, so it **runs without asking**. It creates a new file or appends and can never
    ///   replace or remove content (`WorkspaceRoots::write_file`), so the worst it does is add text inside a folder
    ///   the owner granted.
    /// - `edit`: `write`, risk 2, so it **asks by default**: it changes content that already exists. An owner who
    ///   trusts it (`ADR-0133`) is not asked.
    pub fn definition(tool: &str) -> Result<ToolDefinition, FilesystemToolError> {
        struct Spec {
            id: &'static str,
            title: &'static str,
            description: &'static str,
            input: &'static str,
            output: &'static str,
            effect: ToolEffect,
            risk: u8,
            scope: &'static str,
            reads: bool,
        }
        let spec = match tool {
            READ_TOOL => Spec {
                id: READ_TOOL,
                title: "Read a file",
                description: "Reads one file inside a granted workspace root and returns its contents, bounded.",
                input: READ_INPUT_SCHEMA,
                output: READ_OUTPUT_SCHEMA,
                effect: ToolEffect::ReadOnly,
                risk: 0,
                scope: "files.read",
                reads: true,
            },
            LIST_TOOL => Spec {
                id: LIST_TOOL,
                title: "List a directory",
                description: "Lists the entry names in one directory inside a granted workspace root, sorted.",
                input: LIST_INPUT_SCHEMA,
                output: LIST_OUTPUT_SCHEMA,
                effect: ToolEffect::ReadOnly,
                risk: 0,
                scope: "files.read",
                reads: true,
            },
            WRITE_TOOL => Spec {
                id: WRITE_TOOL,
                title: "Write a file",
                description: "Creates a NEW text file inside a granted workspace root (mode create, the default, \
                              refuses if it exists), or appends text to the end of one (mode append). It never \
                              replaces existing content: use jarvis.files.edit to change a file. Missing folders are \
                              created. At most 4000 characters per call; write a longer file in several appends.",
                input: WRITE_INPUT_SCHEMA,
                output: WRITE_OUTPUT_SCHEMA,
                effect: ToolEffect::Write,
                risk: 1,
                scope: "files.write",
                reads: false,
            },
            EDIT_TOOL => Spec {
                id: EDIT_TOOL,
                title: "Edit a file",
                description: "Replaces one exact piece of text in an existing file inside a granted workspace root. \
                              The text to find must occur exactly once (add surrounding lines to make it unique), \
                              and read the file first so the text matches exactly. The change is applied atomically.",
                input: EDIT_INPUT_SCHEMA,
                output: WRITE_OUTPUT_SCHEMA,
                effect: ToolEffect::Write,
                risk: 2,
                scope: "files.write",
                reads: false,
            },
            other => {
                return Err(FilesystemToolError::UnknownTool {
                    tool: other.to_owned(),
                });
            }
        };
        Ok(ToolDefinition::new(ToolDefinitionParts {
            id: ToolId::new(spec.id)?,
            version: "1.0.0".to_owned(),
            title: spec.title.to_owned(),
            description: spec.description.to_owned(),
            input_schema: schema(spec.input)?,
            output_schema: schema(spec.output)?,
            effects: EffectSet::single(spec.effect),
            risk: spec.risk,
            required_scopes: ScopeSet::single(Scope::new(spec.scope)?),
            // `Policy`: the workspace's threshold decides, so risk 0 and 1 run and risk 2 asks.
            approval: if spec.reads {
                ApprovalPolicy::Auto
            } else {
                ApprovalPolicy::Policy
            },
            timeout_seconds: TIMEOUT_SECONDS,
            // A read may be repeated, so a blind retry is safe. A write is not retried blindly: an append
            // repeated would add its text twice.
            retry: if spec.reads {
                RetryDeclaration {
                    attempts: 2,
                    backoff_ceiling_seconds: 1,
                }
            } else {
                RetryDeclaration::none()
            },
            idempotency: if spec.reads {
                Idempotency::Required
            } else {
                Idempotency::Unsupported
            },
            source: ToolSource::Native,
            availability: Availability::Available,
            // A file inside a granted root is at most internal. Declaring `Internal` rather than `Public` says
            // "not public" without claiming a classification of content this adapter cannot know.
            sensitivity: ToolSensitivity::new(Sensitivity::Internal, Sensitivity::Internal),
        })?)
    }
}
/// Explains why an adapter-level definition could not be produced.
///
/// Separate from [`ToolDefinitionError`] because the first case is not a definition fault at all: an
/// unknown tool name is a caller's mistake, and reporting it as a contract violation would point a
/// reader at the wrong string.
#[derive(Clone, Debug, Eq, thiserror::Error, PartialEq)]
pub enum FilesystemToolError {
    /// A tool name this adapter does not provide was asked for.
    #[error("{tool} is not provided by the filesystem read adapter")]
    UnknownTool {
        /// The tool that was asked for.
        tool: String,
    },
    /// The identifier was rejected.
    #[error(transparent)]
    Id(#[from] crate::identifier::ToolIdError),
    /// A schema constant was rejected.
    #[error(transparent)]
    Schema(#[from] SchemaError),
    /// A required scope was rejected.
    #[error(transparent)]
    Scope(#[from] crate::scope::ScopeError),
    /// The definition was rejected by the contract's own validation.
    #[error(transparent)]
    Definition(#[from] ToolDefinitionError),
}

/// Parses one schema constant under the required dialect.
fn schema(text: &str) -> Result<ToolSchema, SchemaError> {
    if !text.contains(TOOL_SCHEMA_DIALECT) {
        return Err(SchemaError::WrongDialect { found: None });
    }
    let value: Value = serde_json::from_str(text).map_err(|_| SchemaError::NotJson)?;
    ToolSchema::from_value(value)
}

#[async_trait]
impl ToolExecutor for FilesystemTool {
    fn adapter_id(&self) -> &'static str {
        "filesystem-read"
    }

    async fn execute(
        &self,
        request: &ToolExecutionRequest,
    ) -> Result<ToolCallResult, AdapterError> {
        // The tool is matched on its canonical string and an unknown one is refused. A
        // `NotImplemented` for an unrecognized tool is what makes a registry mistake ("a tool was
        // advertised that no adapter can run") visible instead of silent.
        let tool = request.tool().to_string();
        let operation = match tool.as_str() {
            READ_TOOL => Operation::Read,
            LIST_TOOL => Operation::List,
            WRITE_TOOL => Operation::Write,
            EDIT_TOOL => Operation::Edit,
            other => {
                return Err(AdapterError::NotImplemented {
                    tool: other.to_owned(),
                });
            }
        };

        // The deadline is checked before any read: a call already past its deadline must not start
        // work, and reporting that as a refusal ("nothing happened") rather than as a failure is the
        // honest choice.
        if request.is_past_deadline(UtcTimestamp::now(&SystemClock)) {
            return Err(AdapterError::RefusedBeforeReaching {
                reason: "the call was already past its deadline".to_owned(),
            });
        }

        let path = path_argument(request)?;
        // Both calls are synchronous and blocking, run on the calling thread. `tokio`'s `fs` feature
        // is not enabled for this crate, and a read of a granted local file is not the long operation
        // a blocking-pool hand-off exists for. Resource limits for a slow mount belong to `P3-011`,
        // which owns sandboxing.
        let outcome = match operation {
            Operation::List => self.list_bounded(&path),
            Operation::Read => self.read_bounded(&path),
            Operation::Write => self.write_bounded(request, &path),
            Operation::Edit => self.edit_bounded(request, &path),
        };
        match outcome {
            Ok(result) => result,
            Err(error) => refuse(&describe_io_error(&error)),
        }
    }
}

/// Which of the four tools a call is for.
#[derive(Clone, Copy)]
enum Operation {
    Read,
    List,
    Write,
    Edit,
}

impl FilesystemTool {
    /// Creates a file or appends to one, and reports what happened.
    ///
    /// Every argument problem is refused **before reaching** the filesystem. The write itself resolves beneath a
    /// root's handle, so a path that would leave the root fails exactly as a read does.
    fn write_bounded(
        &self,
        request: &ToolExecutionRequest,
        path: &Path,
    ) -> Result<Result<ToolCallResult, AdapterError>, std::io::Error> {
        let content = match text_argument(request, "content", MAX_WRITE_CHARS, true) {
            Ok(text) => text,
            Err(error) => return Ok(Err(error)),
        };
        let append = match request.arguments().get("mode") {
            None => false,
            Some(Value::String(mode)) if mode == "create" => false,
            Some(Value::String(mode)) if mode == "append" => true,
            Some(_) => {
                return Ok(Err(AdapterError::RefusedBeforeReaching {
                    reason: "the mode argument must be \"create\" or \"append\"".to_owned(),
                }));
            }
        };
        let (created, root) = self.roots.write_file(path, content.as_bytes(), append)?;
        let body = json!({
            "outcome": if created { "created" } else { "appended" },
            "bytes": content.len(),
        });
        Ok(confirm(
            &locator("file", path, root),
            BoundedOutput::from_bounded(body.to_string(), false),
        ))
    }

    /// Replaces one exact piece of text in an existing file, atomically.
    fn edit_bounded(
        &self,
        request: &ToolExecutionRequest,
        path: &Path,
    ) -> Result<Result<ToolCallResult, AdapterError>, std::io::Error> {
        let find = match text_argument(request, "find", MAX_EDIT_FIND_CHARS, false) {
            Ok(text) => text,
            Err(error) => return Ok(Err(error)),
        };
        let replace = match text_argument(request, "replace", MAX_WRITE_CHARS, true) {
            Ok(text) => text,
            Err(error) => return Ok(Err(error)),
        };
        if find == replace {
            return Ok(Err(AdapterError::RefusedBeforeReaching {
                reason: "the replacement is identical to the text it replaces".to_owned(),
            }));
        }
        let root = self
            .roots
            .replace_text(path, MAX_EDITABLE_FILE_BYTES, |text| {
                replace_once(text, &find, &replace)
            })?;
        let body = json!({ "outcome": "edited", "bytes": replace.len() });
        Ok(confirm(
            &locator("file", path, root),
            BoundedOutput::from_bounded(body.to_string(), false),
        ))
    }

    /// Reads a bounded prefix of a file and reports it.
    ///
    /// The inner `Result` is the adapter's vocabulary and the outer one is I/O, so the caller can map
    /// an I/O failure to an honest outcome without this function needing to know that mapping.
    fn read_bounded(
        &self,
        path: &Path,
    ) -> Result<Result<ToolCallResult, AdapterError>, std::io::Error> {
        let (file, root) = self.roots.open_file(path)?;
        // One byte past the bound is read so "exactly the bound" and "more than the bound" are
        // distinguishable: stopping at the bound cannot tell a file of exactly the limit from a larger
        // one, and that difference is the `truncated` flag.
        let mut reader = file.take((MAX_TOOL_OUTPUT_BYTES as u64) + 1);
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;

        let truncated = buffer.len() > MAX_TOOL_OUTPUT_BYTES;
        if truncated {
            buffer.truncate(MAX_TOOL_OUTPUT_BYTES);
        }
        // The cut may land inside a multi-byte character, so the bytes are decoded as UTF-8 and the
        // valid prefix is kept. **Empty is not acceptable here**, and this is a defect a test found:
        // from_utf8 reports the valid prefix, and a file of invalid bytes from the first byte has a
        // valid prefix of zero, which decodes to an empty String -- perfectly valid UTF-8 -- and was
        // reported as a `Confirmed` read of nothing. A file that is not text is refused rather than
        // returned as silent emptiness, and it is refused rather than lossily converted, which would
        // replace content with U+FFFD and make a mangled read look like a successful one.
        let text = match String::from_utf8(buffer) {
            Ok(text) => text,
            Err(error) => {
                let valid = error.utf8_error().valid_up_to();
                if valid == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "the file does not begin with UTF-8 text",
                    ));
                }
                let mut bytes = error.into_bytes();
                bytes.truncate(valid);
                String::from_utf8(bytes).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "the file is not valid UTF-8 text",
                    )
                })?
            }
        };

        let evidence = locator("file", path, root);
        Ok(confirm(
            &evidence,
            BoundedOutput::from_bounded(text, truncated),
        ))
    }

    /// Lists a directory's entries and reports them as JSON.
    fn list_bounded(
        &self,
        path: &Path,
    ) -> Result<Result<ToolCallResult, AdapterError>, std::io::Error> {
        let (mut names, root) = self.roots.read_directory(path)?;
        let truncated = names.len() > MAX_LISTED_ENTRIES;
        if truncated {
            names.truncate(MAX_LISTED_ENTRIES);
        }
        let text = serde_json::to_string(&json!({
            "entries": names,
            "truncated": truncated,
        }))
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error.to_string()))?;

        let evidence = locator("directory", path, root);
        Ok(confirm(
            &evidence,
            BoundedOutput::from_bounded(text, truncated),
        ))
    }
}

/// Replaces `find` by `replace` when it occurs exactly once, and says why when it does not.
///
/// A file with Windows line endings is matched by a `find` written with plain newlines (and the replacement is
/// converted the same way), because a model reading the file sees lines and not their terminators.
fn replace_once(text: &str, find: &str, replace: &str) -> Result<String, std::io::Error> {
    let invalid = |message: String| std::io::Error::new(std::io::ErrorKind::InvalidInput, message);
    let (needle, replacement) =
        if text.contains(find) || !text.contains("\r\n") || find.contains('\r') {
            (find.to_owned(), replace.to_owned())
        } else {
            (find.replace('\n', "\r\n"), replace.replace('\n', "\r\n"))
        };
    match text.matches(needle.as_str()).count() {
        0 => Err(invalid(
            "the text to replace was not found; read the file again and copy the text exactly"
                .to_owned(),
        )),
        1 => Ok(text.replacen(needle.as_str(), &replacement, 1)),
        many => Err(invalid(format!(
            "the text to replace occurs {many} times; include more surrounding lines so it matches once"
        ))),
    }
}

/// Reads a text argument, refusing one that is missing, too long, empty when it must not be, or holding a NUL.
fn text_argument(
    request: &ToolExecutionRequest,
    name: &str,
    max_chars: usize,
    may_be_empty: bool,
) -> Result<String, AdapterError> {
    let Some(Value::String(text)) = request.arguments().get(name) else {
        return Err(AdapterError::RefusedBeforeReaching {
            reason: format!("the {name} argument is missing or is not a string"),
        });
    };
    if text.is_empty() && !may_be_empty {
        return Err(AdapterError::RefusedBeforeReaching {
            reason: format!("the {name} argument is empty"),
        });
    }
    if text.chars().count() > max_chars {
        return Err(AdapterError::RefusedBeforeReaching {
            reason: format!("the {name} argument exceeds {max_chars} characters"),
        });
    }
    if text.contains('\0') {
        return Err(AdapterError::RefusedBeforeReaching {
            reason: format!("the {name} argument contains a NUL byte"),
        });
    }
    Ok(text.clone())
}

/// Builds the evidence locator for a resolved path.
///
/// The locator is `<kind>:<root>/<relative path>` rather than the content, which is the point of
/// separate evidence and output: `docs/architecture/tools-and-connectors.md` requires provider
/// identifiers preserved separately from user-facing text, and the resolvable location is what makes
/// a `Confirmed` outcome checkable later.
fn locator(kind: &str, path: &Path, root: &Path) -> String {
    format!("{kind}:{}/{}", root.display(), path.display())
}

/// Builds a `Confirmed` result, degrading to an honest `Failed` when the locator is unusable.
///
/// `ProviderEvidence::new` refuses an empty value, which a locator cannot be — but returning an error
/// rather than asserting keeps this total. A failure to build evidence is reported as `Failed` with a
/// reason rather than as `Confirmed`, because a confirmation with no evidence is exactly what
/// [`ToolOutcomeRecord::confirmed`] refuses.
fn confirm(evidence: &str, output: BoundedOutput) -> Result<ToolCallResult, AdapterError> {
    let Ok(evidence) = ProviderEvidence::new(evidence) else {
        return refuse("the resolved path was unusable as evidence");
    };
    let Ok(record) = ToolOutcomeRecord::confirmed(evidence.as_str()) else {
        return refuse("the resolved path was unusable as evidence");
    };
    Ok(ToolCallResult::new(
        record,
        Some(evidence),
        Some(output),
        UtcTimestamp::now(&SystemClock),
    ))
}

/// Reads the path argument, refusing one that is absent, unusable, or oversized.
///
/// # Errors
///
/// Returns [`AdapterError::RefusedBeforeReaching`] — deliberately not `ProviderRefused`: a malformed
/// argument never reaches the filesystem, so "nothing happened at all" is the accurate statement, and
/// an adapter must only make that statement when it is certain.
fn path_argument(request: &ToolExecutionRequest) -> Result<PathBuf, AdapterError> {
    let Some(Value::String(text)) = request.arguments().get("path") else {
        return Err(AdapterError::RefusedBeforeReaching {
            reason: "the path argument is missing or is not a string".to_owned(),
        });
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(AdapterError::RefusedBeforeReaching {
            reason: "the path argument is empty".to_owned(),
        });
    }
    if trimmed.chars().count() > MAX_PATH_ARGUMENT_CHARS {
        return Err(AdapterError::RefusedBeforeReaching {
            reason: format!("the path argument exceeds {MAX_PATH_ARGUMENT_CHARS} characters"),
        });
    }
    // A NUL cannot appear in a real path and truncates a C string, so it is refused here rather than
    // handed to the platform layer as a shorter path than the one that was bounded above.
    if trimmed.contains('\0') {
        return Err(AdapterError::RefusedBeforeReaching {
            reason: "the path argument contains a NUL byte".to_owned(),
        });
    }
    Ok(PathBuf::from(trimmed))
}

/// Reports a refusal as an honest `Failed` outcome with a bounded reason.
///
/// A refusal to serve a path **has reached the filesystem** — the resolution was attempted and failed
/// — so it is not [`AdapterError::RefusedBeforeReaching`], which means nothing happened at all.
/// `Failed` is the honest split: the call did run, and it established that no effect occurred and
/// nothing was read.
fn refuse(reason: &str) -> Result<ToolCallResult, AdapterError> {
    let bounded: String = reason.chars().take(MAX_OUTCOME_DETAIL_CHARS).collect();
    let Ok(record) = ToolOutcomeRecord::failed(bounded) else {
        return Err(AdapterError::ProviderRefused {
            reason: "the refusal reason was unusable".to_owned(),
        });
    };
    Ok(ToolCallResult::new(
        record,
        None,
        None,
        UtcTimestamp::now(&SystemClock),
    ))
}

/// Renders an I/O error as a bounded, actionable reason.
///
/// The **kind** is included because that is the part a reader can act on, and the raw message can
/// contain a path the caller already supplied. It is deliberately not flattened into "denied": a
/// missing file and a resolution that left the workspace arrive here as different kinds, and
/// collapsing them would hide the difference between a typo and an escape attempt from the reader who
/// needs to see it.
fn describe_io_error(error: &std::io::Error) -> String {
    let kind = match error.kind() {
        std::io::ErrorKind::NotFound => "not found",
        std::io::ErrorKind::PermissionDenied => "not permitted",
        std::io::ErrorKind::InvalidData => "not readable as text",
        std::io::ErrorKind::IsADirectory => "is a directory",
        std::io::ErrorKind::AlreadyExists => "already exists",
        std::io::ErrorKind::InvalidInput => "refused",
        std::io::ErrorKind::NotADirectory => "is not a directory",
        _ => "the filesystem refused the request",
    };
    format!("{kind}: {error}")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use jarvis_core::{CorrelationId, ToolOutcome};
    use serde_json::json;

    use super::*;
    use crate::execution::{AuthorizationReceipt, AuthorizationReceiptParts, IdempotencyKey};
    use crate::executor::ToolExecutionRequestParts;
    use crate::risk::Risk;
    use crate::workspace::WorkspaceRoots;

    /// A temporary directory removed when the test ends, pass or fail.
    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("jarvis-files-tool-{}", jarvis_core::scratch_tag()));
            fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create root: {error}"));
            // Canonicalized because `temp_dir()` on Windows can return an 8.3 short form whose text
            // differs from the long form a handle resolution reports.
            let path = path
                .canonicalize()
                .unwrap_or_else(|error| panic!("canonicalize root: {error}"));
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
            }
            fs::write(&path, contents)
                .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
            path
        }

        fn make_dir(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(&path)
                .unwrap_or_else(|error| panic!("create dir {}: {error}", path.display()));
            path
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            jarvis_core::remove_scratch_dir(&self.0);
        }
    }

    /// Creates a directory link at `link` pointing at `target`.
    ///
    /// A **junction** on Windows rather than a symlink, and the choice is the test's point: creating a
    /// Windows symlink needs `SeCreateSymbolicLinkPrivilege` or Developer Mode, which an unprivileged
    /// user typically lacks, while any user can create a junction. Testing a primitive the attacker
    /// often cannot use would be testing the wrong escape vector.
    ///
    /// A failure to create the link panics rather than skipping. A skipped link test passes while
    /// proving nothing, which is the failure mode this module exists to avoid.
    fn link_directory(target: &Path, link: &Path) {
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd")
                .arg("/C")
                .arg("mklink")
                .arg("/J")
                .arg(link)
                .arg(target)
                .output()
                .unwrap_or_else(|error| panic!("run mklink: {error}"));
            assert!(
                output.status.success(),
                "a junction must be creatable by any user, so this is a broken fixture: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
                .unwrap_or_else(|error| panic!("create symlink: {error}"));
        }
    }

    /// A timestamp `offset_seconds` from the **real current time**.
    ///
    /// Anchored to the clock rather than to a fixed calendar instant, and the reason is a bug this
    /// test module had: the adapter's deadline check reads the real clock, so a fixture based on a
    /// hardcoded date silently becomes "already lapsed" once that date passes, and every read fails
    /// with a deadline refusal instead of the thing under test. A relative offset cannot drift.
    fn at(offset_seconds: i128) -> UtcTimestamp {
        let now = UtcTimestamp::now(&SystemClock);
        UtcTimestamp::from_unix_nanos(now.unix_nanos() + offset_seconds * 1_000_000_000)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// An allowing decision for one of this adapter's tools, produced by `evaluate`.
    ///
    /// Through the policy engine rather than fabricated, because that is the whole point of the seam
    /// this fixture exercises: an adapter's authority must come from a decision, and the receipt type
    /// now refuses to be built from anything else.
    fn allowing(tool: &str) -> crate::PolicyDecision {
        use crate::evaluation::{
            ActorAuthority, PolicyRequest, TargetAssessment, WorkspacePolicy, evaluate,
        };
        use crate::scope::ScopeSet;

        let definition = FilesystemTool::definition(tool).unwrap_or_else(|error| panic!("{error}"));
        let decision = evaluate(&PolicyRequest {
            definition: &definition,
            actor: ActorAuthority::active(ScopeSet::new([
                Scope::new("files.read").unwrap_or_else(|error| panic!("{error}"))
            ])),
            workspace: &WorkspacePolicy::default(),
            available: true,
            target: TargetAssessment::none(),
        });
        assert!(
            decision.is_allowed(),
            "the adapter's read tools must be allowed by the default workspace, got {:?}",
            decision.reason_code()
        );
        decision
    }

    /// A **denying** decision, for the test that an unknown tool is refused before anything runs.
    ///
    /// An unknown tool has no definition to evaluate, so the decision cannot come from policy over
    /// that tool. It is built as a denial of a *real* read definition instead, which is what a caller
    /// holding a mismatched decision would produce.
    ///
    /// Not currently called: the request fixture now always evaluates against a known tool, because an
    /// unknown tool has no definition. Kept because it is the fixture a future test of the
    /// mismatch path wants, and removing it would make that test re-derive the same construction.
    #[expect(dead_code, reason = "the fixture for a future mismatch test")]
    fn denying(tool: &str) -> crate::PolicyDecision {
        use crate::evaluation::{
            ActorAuthority, PolicyRequest, TargetAssessment, WorkspacePolicy, evaluate,
        };
        use crate::scope::ScopeSet;

        // A workspace that denies this tool outright.
        let definition = FilesystemTool::definition(tool).unwrap_or_else(|error| panic!("{error}"));
        let workspace = WorkspacePolicy::default().denying(definition.id().clone());
        let decision = evaluate(&PolicyRequest {
            definition: &definition,
            actor: ActorAuthority::active(ScopeSet::none()),
            workspace: &workspace,
            available: true,
            target: TargetAssessment::none(),
        });
        assert!(
            decision.is_denied(),
            "the fixture needs a denying decision, got {:?}",
            decision.reason_code()
        );
        decision
    }

    fn request(tool: &str, arguments: Value, deadline: UtcTimestamp) -> ToolExecutionRequest {
        let tool_id = ToolId::new(tool).unwrap_or_else(|error| panic!("{error}"));
        // The digest is computed from the arguments the request will carry, and the decision comes
        // from a real evaluation: `AuthorizationReceipt::new` verifies both, so a fixture cannot
        // stand in a placeholder digest or invent authority.
        //
        // The decision is evaluated against a **known** tool's definition even when `tool` is not one
        // this adapter provides, because the test for an unknown tool needs a receipt that is validly
        // built and names a tool the adapter cannot run. That is precisely the registry mistake the
        // test reproduces: the authority is real, and the tool is missing.
        let intent_hash = jarvis_core::CanonicalIntentHash::compute(tool, "1.0.0", &arguments)
            .unwrap_or_else(|error| panic!("{error}"));
        let receipt = AuthorizationReceipt::new(AuthorizationReceiptParts {
            receipt_id: "0198f000-0000-7000-8000-0000000000e1".to_owned(),
            tool: tool_id.clone(),
            tool_version: "1.0.0".to_owned(),
            arguments: arguments.clone(),
            intent_hash,
            policy_version: "policy-3".to_owned(),
            decision: allowing(READ_TOOL),
            approval: None,
            correlation_id: CorrelationId::new(),
            issued_at: at(0),
        })
        .unwrap_or_else(|error| panic!("{error}"));
        ToolExecutionRequest::new(ToolExecutionRequestParts {
            call_id: "0198f000-0000-7000-8000-0000000000e3".to_owned(),
            tool: tool_id,
            tool_version: "1.0.0".to_owned(),
            arguments,
            receipt,
            idempotency_key: IdempotencyKey::generate().unwrap_or_else(|error| panic!("{error}")),
            deadline,
            correlation_id: CorrelationId::new(),
        })
        .unwrap_or_else(|error| panic!("{error}"))
    }

    fn adapter(root: &Path) -> FilesystemTool {
        let roots = WorkspaceRoots::new([root])
            .unwrap_or_else(|error| panic!("roots must be accepted: {error}"));
        FilesystemTool::new(roots)
    }

    fn run(
        executor: &FilesystemTool,
        tool: &str,
        arguments: Value,
    ) -> Result<ToolCallResult, AdapterError> {
        let request = request(tool, arguments, at(3600));
        block_on(executor.execute(&request))
    }

    /// Drives a future to completion on a shared current-thread runtime.
    ///
    /// Not `#[tokio::test]`, and not the usual trick of polling with a no-op waker either: the
    /// workspace **forbids** `unsafe_code`, so a hand-built `RawWaker` cannot be written here at all.
    /// And not a runtime per call, because the adapter never awaits anything that needs a reactor —
    /// every filesystem call is synchronous inside — so one shared runtime is enough and creating
    /// several would only add setup noise.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        use std::sync::OnceLock;
        use tokio::runtime::Runtime;

        static RUNTIME: OnceLock<Runtime> = OnceLock::new();
        let runtime = RUNTIME.get_or_init(|| {
            // A current-thread runtime, because the dev-dependency enables `rt` rather than
            // `rt-multi-thread` and nothing here schedules concurrent work in the first place.
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap_or_else(|error| panic!("build a test runtime: {error}"))
        });
        runtime.block_on(future)
    }

    /// **A real read succeeds and confirms with a checkable locator, not a bare success.**
    ///
    /// The evidence assertion is the one that matters: `docs/architecture/tools-and-connectors.md`
    /// forbids turning a success-sounding string into proof, so a `Confirmed` outcome must carry the
    /// locator that makes it checkable rather than only the contents the model sees.
    #[test]
    fn a_read_returns_bounded_content_and_a_locator() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");
        directory.write("root/notes/todo.txt", "buy milk");

        let executor = adapter(&root);
        let result = run(&executor, READ_TOOL, json!({"path": "notes/todo.txt"}))
            .unwrap_or_else(|error| panic!("read must succeed: {error}"));

        assert_eq!(result.outcome(), ToolOutcome::Confirmed);
        let output = result
            .output()
            .unwrap_or_else(|| panic!("a read must return output"));
        assert!(output.content().contains("buy milk"));
        assert!(!output.is_truncated());
        assert!(
            result
                .evidence()
                .unwrap_or_else(|| panic!("a confirmation must carry evidence"))
                .as_str()
                .starts_with("file:"),
            "the evidence must be a locator, which is what makes it checkable"
        );
    }

    /// A listing returns sorted entry names as JSON and confirms.
    #[test]
    fn a_listing_returns_sorted_entries() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");
        for name in ["charlie.txt", "alpha.txt", "bravo.txt"] {
            directory.write(&format!("root/{name}"), "x");
        }

        let executor = adapter(&root);
        let result = run(&executor, LIST_TOOL, json!({"path": "."}))
            .unwrap_or_else(|error| panic!("list must succeed: {error}"));

        assert_eq!(result.outcome(), ToolOutcome::Confirmed);
        let output = result
            .output()
            .unwrap_or_else(|| panic!("a listing must return output"));
        let parsed: Value = serde_json::from_str(output.content())
            .unwrap_or_else(|error| panic!("the listing must be JSON: {error}"));
        let entries = parsed
            .get("entries")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("entries must be an array: {parsed}"));
        let names: Vec<&str> = entries.iter().filter_map(Value::as_str).collect();
        assert_eq!(names, vec!["alpha.txt", "bravo.txt", "charlie.txt"]);
        assert_eq!(parsed.get("truncated"), Some(&Value::Bool(false)));
    }

    /// **Oversized output is truncated at the bound, and the flag says so.**
    ///
    /// The file is several times the bound, so the limited read is genuinely exercised: a test whose
    /// file is just over the limit would pass with an implementation that read everything and cut it
    /// afterwards, which is the allocation the limited reader exists to avoid.
    #[test]
    fn an_oversized_file_is_truncated_and_marked() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");
        let huge = "a".repeat(MAX_TOOL_OUTPUT_BYTES * 4);
        directory.write("root/huge.txt", &huge);

        let executor = adapter(&root);
        let result = run(&executor, READ_TOOL, json!({"path": "huge.txt"}))
            .unwrap_or_else(|error| panic!("read must succeed: {error}"));

        assert_eq!(result.outcome(), ToolOutcome::Confirmed);
        let output = result
            .output()
            .unwrap_or_else(|| panic!("a read must return output"));
        assert!(
            output.is_truncated(),
            "content four times the bound must be marked truncated"
        );
        assert_eq!(output.byte_len(), MAX_TOOL_OUTPUT_BYTES);
    }

    /// **A file of exactly the bound is NOT marked truncated.**
    ///
    /// The boundary in the other direction: reading one byte past the bound is what makes a file of
    /// exactly the limit distinguishable from a larger one, and a test that only checked the oversized
    /// case would pass with an implementation that always reported truncation.
    #[test]
    fn a_file_of_exactly_the_bound_is_not_truncated() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");
        directory.write("root/exact.txt", &"b".repeat(MAX_TOOL_OUTPUT_BYTES));

        let executor = adapter(&root);
        let result = run(&executor, READ_TOOL, json!({"path": "exact.txt"}))
            .unwrap_or_else(|error| panic!("read must succeed: {error}"));

        let output = result
            .output()
            .unwrap_or_else(|| panic!("a read must return output"));
        assert!(
            !output.is_truncated(),
            "a file of exactly the bound fits and must not be marked truncated"
        );
        assert_eq!(output.byte_len(), MAX_TOOL_OUTPUT_BYTES);
    }

    /// **A traversal attempt fails as `Failed` — not as a refusal, and not as a success.**
    ///
    /// The outcome split is the assertion worth making. `RefusedBeforeReaching` would claim nothing
    /// happened *at all*, which is false: the filesystem was reached and said no. A success is the
    /// escape. `Failed` is the honest middle.
    #[test]
    fn a_traversal_attempt_fails_and_reads_nothing() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        outer.write("outside.txt", "outside the grant");

        let executor = adapter(&root);
        let result = run(&executor, READ_TOOL, json!({"path": "../outside.txt"}))
            .unwrap_or_else(|error| panic!("a refusal is a result, not an adapter error: {error}"));

        assert_eq!(result.outcome(), ToolOutcome::Failed);
        assert!(
            result.output().is_none(),
            "a refused read must return no content at all"
        );
        assert!(
            result.evidence().is_none(),
            "a refused read has nothing to confirm"
        );
        let reason = result
            .record()
            .reason()
            .unwrap_or_else(|| panic!("a failure must carry a reason"));
        assert!(
            !reason.contains("outside the grant"),
            "the reason must not echo file content: {reason}"
        );
    }

    /// **A directory link out of the root cannot be read through the adapter.**
    ///
    /// The end-to-end form of the confinement test: the same escape, driven through the tool rather
    /// than through the boundary, so a bug in the adapter's path handling could not be the thing that
    /// made the boundary test pass.
    #[test]
    fn a_link_out_of_the_root_cannot_be_read_through_the_tool() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        outer.write("outside/secret.txt", "outside the grant");
        link_directory(&outer.path().join("outside"), &root.join("escape"));

        // The control: ambient authority reaches it, so the link is real.
        assert!(
            fs::read_to_string(root.join("escape").join("secret.txt")).is_ok(),
            "the escape must be real for ambient authority or this test proves nothing"
        );

        let executor = adapter(&root);
        let result = run(&executor, READ_TOOL, json!({"path": "escape/secret.txt"}))
            .unwrap_or_else(|error| panic!("a refusal is a result, not an adapter error: {error}"));

        assert_eq!(result.outcome(), ToolOutcome::Failed);
        assert!(
            result.output().is_none(),
            "nothing may be read through a link out of the root"
        );

        // And the positive control at the same time: a real file in the root still reads, so the
        // failure above is about the link rather than about the adapter being broken.
        outer.write("root/inside.txt", "inside the grant");
        let inside = run(&executor, READ_TOOL, json!({"path": "inside.txt"}))
            .unwrap_or_else(|error| panic!("read must succeed: {error}"));
        assert_eq!(inside.outcome(), ToolOutcome::Confirmed);
    }

    fn write_args(path: &str, content: &str, append: bool) -> Value {
        json!({"path": path, "content": content, "mode": if append { "append" } else { "create" }})
    }

    fn edit_args(path: &str, find: &str, replace: &str) -> Value {
        json!({"path": path, "find": find, "replace": replace})
    }

    fn failure_reason(result: &ToolCallResult) -> String {
        assert_eq!(result.outcome(), ToolOutcome::Failed);
        result
            .record()
            .reason()
            .unwrap_or_else(|| panic!("a failure must carry a reason"))
            .to_owned()
    }

    #[test]
    fn write_and_edit_are_declared_as_writes_with_the_intended_risk() {
        let write = FilesystemTool::definition(WRITE_TOOL).unwrap_or_else(|e| panic!("{e}"));
        let edit = FilesystemTool::definition(EDIT_TOOL).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(write.effects(), &EffectSet::single(ToolEffect::Write));
        assert_eq!(edit.effects(), &EffectSet::single(ToolEffect::Write));
        assert_eq!(write.risk(), Risk::Low);
        assert_eq!(edit.risk(), Risk::Moderate);
        assert!(
            write.approval().is_runnable(),
            "creating a file runs by default"
        );
    }

    #[test]
    fn write_creates_a_new_file_and_its_parents() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        let executor = adapter(&root);

        let result = run(
            &executor,
            WRITE_TOOL,
            write_args("notes/today/plan.txt", "ship it", false),
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(result.outcome(), ToolOutcome::Confirmed);
        assert_eq!(
            fs::read_to_string(root.join("notes").join("today").join("plan.txt"))
                .unwrap_or_default(),
            "ship it"
        );
    }

    #[test]
    fn write_never_replaces_an_existing_file() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        outer.write("root/keep.txt", "original");
        let executor = adapter(&root);

        let result = run(
            &executor,
            WRITE_TOOL,
            write_args("keep.txt", "clobber", false),
        )
        .unwrap_or_else(|error| panic!("{error}"));

        let reason = failure_reason(&result);
        assert!(
            reason.contains("exists"),
            "the reason must say why: {reason}"
        );
        assert_eq!(
            fs::read_to_string(root.join("keep.txt")).unwrap_or_default(),
            "original"
        );
    }

    #[test]
    fn write_can_append_to_an_existing_file() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        outer.write("root/log.txt", "one\n");
        let executor = adapter(&root);

        let result = run(&executor, WRITE_TOOL, write_args("log.txt", "two\n", true))
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            result.outcome(),
            ToolOutcome::Confirmed,
            "{:?}",
            result.record().reason()
        );
        assert_eq!(
            fs::read_to_string(root.join("log.txt")).unwrap_or_default(),
            "one\ntwo\n"
        );
    }

    /// **Writing cannot leave the granted folder**: traversal, an absolute path and a link are all refused,
    /// and nothing is created outside.
    #[test]
    fn write_cannot_escape_the_root() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        outer.make_dir("outside");
        link_directory(&outer.path().join("outside"), &root.join("escape"));
        let executor = adapter(&root);

        for path in [
            "../escaped.txt",
            "escape/escaped.txt",
            "C:/escaped.txt",
            "/escaped.txt",
        ] {
            let result = run(&executor, WRITE_TOOL, write_args(path, "x", false))
                .unwrap_or_else(|error| panic!("a refusal is a result: {error}"));
            assert_eq!(
                result.outcome(),
                ToolOutcome::Failed,
                "{path} must be refused"
            );
        }
        assert!(!outer.path().join("escaped.txt").exists());
        assert!(!outer.path().join("outside").join("escaped.txt").exists());

        let inside = run(&executor, WRITE_TOOL, write_args("inside.txt", "x", false))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            inside.outcome(),
            ToolOutcome::Confirmed,
            "the control must still write"
        );
    }

    #[test]
    fn write_refuses_oversized_content() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        let executor = adapter(&root);
        let big = "a".repeat(MAX_WRITE_CHARS + 1);

        let refused = run(&executor, WRITE_TOOL, write_args("big.txt", &big, false));
        let refused = refused.map_or(true, |result| result.outcome() == ToolOutcome::Failed);
        assert!(refused);
        assert!(!root.join("big.txt").exists());
    }

    #[test]
    fn edit_replaces_one_unique_text_and_leaves_no_temporary_file() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        outer.write("root/app.txt", "alpha\nbeta\ngamma\n");
        let executor = adapter(&root);

        let result = run(&executor, EDIT_TOOL, edit_args("app.txt", "beta", "BETA"))
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(result.outcome(), ToolOutcome::Confirmed);
        assert_eq!(
            fs::read_to_string(root.join("app.txt")).unwrap_or_default(),
            "alpha\nBETA\ngamma\n"
        );
        let leftovers: Vec<_> = fs::read_dir(&root)
            .unwrap_or_else(|error| panic!("{error}"))
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, vec!["app.txt".to_owned()]);
    }

    #[test]
    fn edit_refuses_text_that_is_missing_or_ambiguous_and_changes_nothing() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        outer.write("root/app.txt", "x = 1\nx = 1\ny = 2\n");
        let executor = adapter(&root);

        let missing = run(&executor, EDIT_TOOL, edit_args("app.txt", "z = 3", "z = 4"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(failure_reason(&missing).contains("not found"));

        let twice = run(&executor, EDIT_TOOL, edit_args("app.txt", "x = 1", "x = 9"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            failure_reason(&twice).contains('2'),
            "the count must be named"
        );

        let same = run(&executor, EDIT_TOOL, edit_args("app.txt", "y = 2", "y = 2"));
        assert!(
            same.is_err(),
            "an identical replacement is refused before reaching the file"
        );

        assert_eq!(
            fs::read_to_string(root.join("app.txt")).unwrap_or_default(),
            "x = 1\nx = 1\ny = 2\n"
        );
    }

    #[test]
    fn edit_matches_across_crlf_line_endings() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        fs::write(root.join("win.txt"), "one\r\ntwo\r\nthree\r\n")
            .unwrap_or_else(|e| panic!("{e}"));
        let executor = adapter(&root);

        let result = run(
            &executor,
            EDIT_TOOL,
            edit_args("win.txt", "one\ntwo", "uno\ndos"),
        )
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(result.outcome(), ToolOutcome::Confirmed);
        assert_eq!(
            fs::read_to_string(root.join("win.txt")).unwrap_or_default(),
            "uno\r\ndos\r\nthree\r\n"
        );
    }

    #[test]
    fn edit_cannot_reach_a_file_outside_the_root() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        outer.write("outside/secret.txt", "keep");
        link_directory(&outer.path().join("outside"), &root.join("escape"));
        outer.write("outside.txt", "keep");
        let executor = adapter(&root);

        for path in ["../outside.txt", "escape/secret.txt"] {
            let result = run(&executor, EDIT_TOOL, edit_args(path, "keep", "gone"))
                .unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(
                result.outcome(),
                ToolOutcome::Failed,
                "{path} must be refused"
            );
        }
        assert_eq!(
            fs::read_to_string(outer.path().join("outside.txt")).unwrap_or_default(),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(outer.path().join("outside").join("secret.txt")).unwrap_or_default(),
            "keep"
        );
    }
    /// An absolute path is refused, and a missing file is `Failed` rather than an adapter error.
    #[test]
    fn absolute_and_missing_paths_fail_without_reaching_a_host_file() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");

        let executor = adapter(&root);
        for absolute in ["/etc/passwd", r"C:\Windows\win.ini"] {
            let result = run(&executor, READ_TOOL, json!({"path": absolute}))
                .unwrap_or_else(|error| panic!("a refusal is a result: {error}"));
            assert_eq!(
                result.outcome(),
                ToolOutcome::Failed,
                "{absolute} must not read a host file"
            );
        }

        let missing = run(&executor, READ_TOOL, json!({"path": "nope.txt"}))
            .unwrap_or_else(|error| panic!("a refusal is a result: {error}"));
        assert_eq!(missing.outcome(), ToolOutcome::Failed);
    }

    /// A malformed argument is refused **before** any filesystem access.
    ///
    /// `RefusedBeforeReaching` is the right vocabulary here and `Failed` is not: nothing was reached,
    /// so "nothing happened at all" is certain rather than inferred.
    #[test]
    fn a_malformed_argument_is_refused_before_reaching_the_filesystem() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");
        let executor = adapter(&root);

        let cases: Vec<Value> = vec![
            json!({}),
            json!({"path": ""}),
            json!({"path": "   "}),
            json!({"path": 7}),
            json!({"path": null}),
            json!({"path": "a".repeat(MAX_PATH_ARGUMENT_CHARS + 1)}),
        ];
        for arguments in cases {
            let error = run(&executor, READ_TOOL, arguments.clone())
                .err()
                .unwrap_or_else(|| panic!("{arguments} must be refused, not attempted"));
            assert!(
                matches!(error, AdapterError::RefusedBeforeReaching { .. }),
                "{arguments} must be a refusal, got {error:?}"
            );
        }
    }

    /// A call past its deadline is refused without touching the filesystem.
    #[test]
    fn a_lapsed_deadline_is_refused_before_reaching_the_filesystem() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");
        directory.write("root/present.txt", "here");
        let executor = adapter(&root);

        let lapsed = request(READ_TOOL, json!({"path": "present.txt"}), at(-1));
        let error = block_on(executor.execute(&lapsed))
            .err()
            .unwrap_or_else(|| panic!("a past deadline must be refused"));
        assert!(
            matches!(error, AdapterError::RefusedBeforeReaching { .. }),
            "a lapsed deadline must be a refusal, got {error:?}"
        );
    }

    /// **A tool this adapter does not provide is `NotImplemented`, not a silent read.**
    ///
    /// The case that would be dangerous as a default: a registry offering a tool no adapter can run
    /// must be visible, and a fallback to `read` would make a mistyped name read a file instead.
    #[test]
    fn an_unknown_tool_is_not_implemented() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");
        let executor = adapter(&root);

        let error = run(&executor, "jarvis.files.delete", json!({"path": "x"}))
            .err()
            .unwrap_or_else(|| panic!("an unknown tool must be refused"));
        assert!(
            matches!(error, AdapterError::NotImplemented { .. }),
            "got {error:?}"
        );

        // The adapter's id is what an audit record attributes a call to.
        assert_eq!(executor.adapter_id(), "filesystem-read");
    }

    /// **Both definitions are accepted by the contract, and both are read-only and bounded.**
    ///
    /// The check that keeps the schema strings honest: `ToolDefinition::new` rejects a schema that does
    /// not declare the dialect or that is not valid 2020-12, so an authoring mistake in a constant
    /// surfaces here rather than at a call.
    #[test]
    fn both_definitions_are_accepted_and_declare_a_read() {
        let definitions = FilesystemTool::definitions()
            .unwrap_or_else(|error| panic!("both definitions must be accepted: {error}"));
        assert_eq!(definitions.len(), 4);

        for definition in definitions
            .iter()
            .filter(|d| [READ_TOOL, LIST_TOOL].contains(&d.id().to_string().as_str()))
        {
            assert_eq!(
                definition.effects(),
                &EffectSet::single(ToolEffect::ReadOnly),
                "{} must declare only a read",
                definition.id()
            );
            assert_eq!(definition.risk(), Risk::Minimal);
            assert!(
                definition.approval().is_runnable(),
                "a read of granted content must be runnable"
            );
            assert!(
                definition
                    .required_scopes()
                    .iter()
                    .any(|scope| scope.resource() == "files" && scope.action() == "read"),
                "{} must require the read scope",
                definition.id()
            );
        }

        let ids: Vec<String> = definitions.iter().map(|d| d.id().to_string()).collect();
        assert!(ids.contains(&READ_TOOL.to_owned()));
        assert!(ids.contains(&LIST_TOOL.to_owned()));

        // An unknown name is refused rather than defaulting to the read tool.
        assert!(matches!(
            FilesystemTool::definition("jarvis.files.delete"),
            Err(FilesystemToolError::UnknownTool { .. })
        ));
    }

    /// **The falsification: the obvious scheme loses the race, and the handle does not.**
    ///
    /// This is not a test of production code — it is the evidence that the production design is
    /// load-bearing. It runs the *rejected* scheme beside the accepted one on the same fixture, so the
    /// difference is demonstrated rather than argued.
    ///
    /// # What the naive scheme actually gets wrong
    ///
    /// This test was written once with the opposite premise and **failed**, and the failure is worth
    /// recording because the corrected premise is the real argument. The first version asserted that a
    /// pre-existing link defeats a prefix check. On Windows it does not: `fs::canonicalize` resolves a
    /// junction, so `starts_with(root)` is false and the check correctly refuses. The naive scheme's
    /// defect is not a pre-existing link — it is that the check and the open happen at **two different
    /// moments**, and the answer to "is this path inside the root?" can change in between.
    ///
    /// So the test performs the two steps the naive code performs, with the attacker's change between
    /// them — which is exactly what a race is, and it needs no timing luck to demonstrate:
    ///
    /// 1. **check**: canonicalize and confirm the path is inside the root. It is.
    /// 2. **swap**: replace the checked component with a link out of the root.
    /// 3. **open**: the naive open follows the link. The check already passed, and nothing re-checks.
    ///
    /// The handle cannot lose this race, because there is no earlier moment to be fooled: it resolves
    /// once, at the open, beneath its own directory handle.
    #[test]
    fn the_naive_check_and_open_loses_the_race_that_the_handle_does_not() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        // The attacker's directory holds a file at the SAME relative path, so the swap makes
        // `notes/todo.txt` resolve to the attacker's content. A link to a directory with different
        // contents would fail with "not found" and demonstrate nothing.
        outer.write("outside/todo.txt", "outside the grant");
        outer.write("root/notes/todo.txt", "inside the grant");

        let requested = Path::new("notes").join("todo.txt");
        let joined = root.join(&requested);

        // Step 1: the check. The canonical path IS inside the root, so the naive guard is satisfied.
        let canonical_root = fs::canonicalize(&root).unwrap_or_else(|error| panic!("{error}"));
        let canonical =
            fs::canonicalize(&joined).unwrap_or_else(|error| panic!("canonicalize: {error}"));
        assert!(
            canonical.starts_with(&canonical_root),
            "the check must pass before the swap, or this demonstrates nothing"
        );

        // Step 2: the swap, between the check and the use.
        fs::remove_dir_all(root.join("notes"))
            .unwrap_or_else(|error| panic!("remove the checked component: {error}"));
        link_directory(&outer.path().join("outside"), &root.join("notes"));

        // Step 3: the naive open. It follows the link, and the check that passed a moment ago is now
        // describing a path that no longer exists.
        let escaped = fs::read_to_string(&joined)
            .unwrap_or_else(|error| panic!("the naive open must escape: {error}"));
        assert_eq!(
            escaped, "outside the grant",
            "the naive scheme reads outside the root after its check passed"
        );

        // The handle, on the same final state: one resolution, beneath the root, and the link is
        // refused. It did not have to notice the swap, because it never checked anything earlier.
        let executor = adapter(&root);
        let result = run(&executor, READ_TOOL, json!({"path": "notes/todo.txt"}))
            .unwrap_or_else(|error| panic!("a refusal is a result: {error}"));
        assert_eq!(
            result.outcome(),
            ToolOutcome::Failed,
            "the handle must refuse what the naive check allowed"
        );
        assert!(
            result.output().is_none(),
            "nothing may be read through the swapped component"
        );
    }

    /// A binary file fails as `Failed` with a reason rather than returning replacement characters.
    ///
    /// A lossy conversion would return U+FFFD and look like a successful read of mangled text, which
    /// is the "success-sounding" outcome the architecture forbids.
    #[test]
    fn a_binary_file_is_reported_rather_than_lossily_decoded() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");
        // Invalid UTF-8 from the first byte, so the valid prefix is empty.
        fs::write(root.join("binary.bin"), [0xff_u8, 0xfe, 0x00, 0x01])
            .unwrap_or_else(|error| panic!("write: {error}"));

        let executor = adapter(&root);
        let result = run(&executor, READ_TOOL, json!({"path": "binary.bin"}))
            .unwrap_or_else(|error| panic!("a refusal is a result: {error}"));
        assert_eq!(result.outcome(), ToolOutcome::Failed);
        assert!(result.output().is_none());
    }
}
