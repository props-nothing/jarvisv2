//! Validated JSON Schema 2020-12 input and output contracts.
//!
//! # Two different checks, and both are required
//!
//! A tool schema is validated against the **2020-12 metaschema** at construction, and an instance is
//! validated against the schema when a call is made. These are not the same check and neither
//! implies the other:
//!
//! - a schema can be well-formed and still reject every instance a provider sends;
//! - an instance can satisfy a schema that is itself invalid, because an unrecognized keyword is
//!   ignored — so a schema with a typo'd keyword validates **everything**.
//!
//! The second case is why the metaschema check is not optional. A tool whose `input_schema` is
//! `{"typo": "object"}` would accept any input at all, and the run would then call a provider with
//! arguments that fail its own validation.
//!
//! # No remote references
//!
//! The validator is built with `jsonschema`'s default features **disabled** (see
//! `docs/research/integrations/json-schema-validation.md`), so a `$ref` to a URL cannot be fetched
//! and a `$ref` to a file cannot be read. That is deliberate: a tool schema is authored by JARVIS or
//! supplied by a connector manifest, and a `$ref` that fetched a URL would make schema validation an
//! outbound HTTP client inside the tool-call path — the SSRF surface
//! `docs/architecture/security.md` lists, reachable by whoever wrote the schema.
//!
//! The cost is real and is stated rather than hidden: a schema that legitimately needs a shared
//! definition cannot reference it by URL. It must inline the definition or resolve against documents
//! JARVIS supplies. `P3-002` owns assembling that document set.
//!
//! Two independent mechanisms enforce it, deliberately. The crate's feature set removes the
//! resolver, and `.offline()` sets a retriever that refuses every fetch. The second is the one that
//! states the intent at the call site: the feature set is a property of `Cargo.toml` that a future
//! edit can change without touching this file, whereas `.offline()` fails closed here.

use std::fmt;

use jsonschema::{Draft, Validator};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The dialect every tool schema must declare.
///
/// Fixed rather than detected. `docs/architecture/tools-and-connectors.md` names JSON Schema
/// 2020-12, and a tool declaring another dialect would be validated under rules its author did not
/// write against — accepting instances the declared contract rejects, or the reverse.
pub const TOOL_SCHEMA_DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

/// The `$schema` keyword, spelled once.
const SCHEMA_KEYWORD: &str = "$schema";

/// Maximum characters in a tool schema document.
///
/// Bounded because a schema is stored, loaded at startup, and included in discovery output. A
/// provider's generated schema can be enormous, and an unbounded one makes registration cost
/// attacker-controlled.
pub const MAX_TOOL_SCHEMA_BYTES: usize = 64 * 1024;

/// Maximum violations reported for one instance.
///
/// Bounded because the list reaches a model as a tool result. An unbounded list is output the model
/// pays for, and the first few violations already tell it what to fix.
pub const MAX_REPORTED_VIOLATIONS: usize = 16;

/// Explains why a tool schema was rejected.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum SchemaError {
    /// The document was not valid JSON.
    #[error("the tool schema is not valid JSON")]
    NotJson,
    /// The document was not a JSON object.
    #[error("a tool schema must be a JSON object")]
    NotAnObject,
    /// The document exceeded the bounded size.
    #[error("the tool schema exceeds {MAX_TOOL_SCHEMA_BYTES} bytes")]
    TooLarge,
    /// The document did not declare the required dialect.
    #[error("a tool schema must declare $schema = {TOOL_SCHEMA_DIALECT}")]
    WrongDialect {
        /// The dialect the document declared, or `None` when it declared none.
        found: Option<String>,
    },
    /// The document used a reference this build cannot resolve.
    ///
    /// A separate variant because the cause is actionable and specific: the fix is to inline the
    /// definition, not to correct a keyword.
    #[error("a tool schema must not reference an external document: {reference}")]
    ExternalReference {
        /// The refused reference, which is a location and not content.
        reference: String,
    },
    /// The document was not a valid schema under its own dialect.
    #[error("the tool schema is not a valid JSON Schema 2020-12 document")]
    InvalidSchema {
        /// The validator's bounded description of the first problem.
        ///
        /// Retained because this is an **author** error surfaced at registration, not a model-facing
        /// payload: a developer registering a tool needs to know which keyword was wrong, and
        /// withholding it would make this a bug report rather than a message.
        detail: String,
    },
}

/// A validated tool schema.
///
/// Compiles the validator once at construction. Registration is where a schema is checked, so
/// holding a compiled validator means a call pays a validation and not a parse — and a schema that
/// cannot compile cannot be registered at all, so the call path never has to handle that failure.
///
/// Serialization is hand-written to emit the document, and deserialization routes through
/// [`ToolSchema::from_value`] so a stored schema is re-checked on load. A derived `Deserialize`
/// would reconstruct the struct's fields directly and leave the validator unbuilt or unchecked.
#[derive(Clone)]
pub struct ToolSchema {
    document: serde_json::Value,
    validator: Validator,
}

impl Serialize for ToolSchema {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.document.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ToolSchema {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let document = serde_json::Value::deserialize(deserializer)?;
        Self::from_value(document).map_err(serde::de::Error::custom)
    }
}

impl ToolSchema {
    /// Validates a schema document and compiles its validator.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError`] when the document is not JSON, is not an object, is oversized, does
    /// not declare [`TOOL_SCHEMA_DIALECT`], uses an unresolvable reference, or is not a valid
    /// 2020-12 schema.
    pub fn parse(document: &str) -> Result<Self, SchemaError> {
        if document.len() > MAX_TOOL_SCHEMA_BYTES {
            return Err(SchemaError::TooLarge);
        }
        let value: serde_json::Value =
            serde_json::from_str(document).map_err(|_| SchemaError::NotJson)?;
        Self::from_value(value)
    }

    /// Validates an already-parsed schema document.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError`] for the same reasons as [`Self::parse`].
    pub fn from_value(value: serde_json::Value) -> Result<Self, SchemaError> {
        let Some(object) = value.as_object() else {
            return Err(SchemaError::NotAnObject);
        };

        // The dialect is required, not defaulted. A document with no `$schema` is unambiguous only
        // to its author, and defaulting it to 2020-12 would silently reinterpret a document written
        // against an earlier draft — where `exclusiveMinimum` is a boolean rather than a number, so
        // a correct 2019-09 schema would become an invalid 2020-12 one.
        let found = object
            .get(SCHEMA_KEYWORD)
            .and_then(serde_json::Value::as_str);
        match found {
            Some(declared) if declared == TOOL_SCHEMA_DIALECT => {}
            other => {
                return Err(SchemaError::WrongDialect {
                    found: other.map(ToOwned::to_owned),
                });
            }
        }

        reject_external_references(&value)?;

        let validator = jsonschema::options()
            .with_draft(Draft::Draft202012)
            // Belt and braces against a `$ref` becoming a fetch. The walk above already refuses any
            // non-local reference, so this can only fire if that walk is later relaxed — which is
            // exactly when it matters. A retriever that refuses everything makes the network path
            // unreachable rather than merely unused.
            .offline()
            .should_validate_formats(false)
            .build(&value)
            .map_err(|error| SchemaError::InvalidSchema {
                detail: bounded_detail(&error.to_string()),
            })?;

        Ok(Self {
            document: value,
            validator,
        })
    }

    /// Returns the schema document.
    #[must_use]
    pub const fn document(&self) -> &serde_json::Value {
        &self.document
    }

    /// Validates an instance against this schema.
    ///
    /// Returns the violations rather than a boolean, because a caller that only learns "invalid" has
    /// nothing to tell the model, and a model that cannot see what was wrong repeats the call.
    ///
    /// # Errors
    ///
    /// Never returns an error today. The signature is a `Result` because a later slice may add a
    /// bounded-output or depth limit that can fail; changing the signature then would touch every
    /// caller, and returning a `Vec` now would have to be wrapped in a `Result` afterwards anyway.
    pub fn validate(&self, instance: &serde_json::Value) -> Result<ValidationReport, SchemaError> {
        let mut violations = Vec::new();
        for error in self.validator.iter_errors(instance) {
            if violations.len() >= MAX_REPORTED_VIOLATIONS {
                break;
            }
            // The instance path and the keyword only. The validator's message text is deliberately
            // NOT carried: it is library-authored prose, and a violation becomes part of a
            // model-visible tool result, so the payload is a pointer plus a keyword name.
            violations.push(SchemaViolation {
                instance_path: sanitize_pointer(error.instance_path().as_str()),
                keyword: sanitize_keyword(error.kind().keyword()),
            });
        }
        Ok(ValidationReport { violations })
    }
}

impl fmt::Debug for ToolSchema {
    /// Prints the document rather than the compiled validator.
    ///
    /// `Validator` is not `Debug`, and its internals are not what a reader wants anyway: a test that
    /// fails should show the schema.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolSchema")
            .field("document", &self.document)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ToolSchema {
    /// Compares the documents. Two schemas with the same document are the same contract, whatever
    /// the validator's internal representation happens to be.
    fn eq(&self, other: &Self) -> bool {
        self.document == other.document
    }
}

impl Eq for ToolSchema {}

/// One reason an instance did not satisfy a schema.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SchemaViolation {
    /// Where in the instance the violation is, as a JSON Pointer.
    ///
    /// A pointer rather than a message: it is a location, it is what a caller needs to fix the
    /// input, and it cannot carry prose an attacker could have authored.
    pub instance_path: String,
    /// The keyword that rejected it.
    pub keyword: String,
}

/// The violations found while validating one instance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ValidationReport {
    /// The violations, bounded by [`MAX_REPORTED_VIOLATIONS`].
    pub violations: Vec<SchemaViolation>,
}

impl ValidationReport {
    /// Returns whether the instance satisfied the schema.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.violations.is_empty()
    }

    /// Returns the violations.
    #[must_use]
    pub fn violations(&self) -> &[SchemaViolation] {
        &self.violations
    }
}

/// Rejects any `$ref` that is not a local pointer.
///
/// Walked rather than configured, because the validator's behaviour for an unresolvable reference is
/// to *fail the build* — which this module would report as an invalid schema, attributing a
/// deliberate refusal to the author's syntax. Finding the reference first lets the error say what is
/// actually wrong.
fn reject_external_references(value: &serde_json::Value) -> Result<(), SchemaError> {
    match value {
        serde_json::Value::Object(object) => {
            for (key, child) in object {
                // `$id` establishes a base URI for relative references, so a non-local `$id` makes
                // later local-looking references resolve elsewhere. Refused for the same reason.
                //
                // Kept as a nested `if` rather than collapsed into a let-chain: the outer test is a
                // keyword filter over *every* key in the document, and folding it into the chain
                // reads as though the reference extraction were the loop's purpose.
                #[allow(clippy::collapsible_if)]
                if key == "$ref" || key == "$id" {
                    if let Some(reference) = child.as_str()
                        && !is_local_reference(reference)
                    {
                        return Err(SchemaError::ExternalReference {
                            reference: reference.to_owned(),
                        });
                    }
                }
                reject_external_references(child)?;
            }
            Ok(())
        }
        serde_json::Value::Array(items) => {
            for item in items {
                reject_external_references(item)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Returns whether a reference resolves inside the document itself.
///
/// A bare fragment (`#/$defs/name`, or `#` for the root) is local. `#anchor` is a plain-name
/// fragment, which is also local to the document. Anything with a scheme or a path is not.
fn is_local_reference(reference: &str) -> bool {
    // `#` alone is the document root; `#/...` is a pointer into it; `#name` is a plain-name
    // fragment. All three resolve within the document.
    reference.starts_with('#')
}

/// Bounds a validator message so a pathological schema cannot produce an unbounded error string.
fn bounded_detail(detail: &str) -> String {
    const MAX_DETAIL_CHARS: usize = 200;
    if detail.chars().count() <= MAX_DETAIL_CHARS {
        return detail.to_owned();
    }
    detail.chars().take(MAX_DETAIL_CHARS).collect::<String>() + "…"
}

/// Keeps a pointer to its safe characters.
///
/// A property name in an instance is caller-controlled and reaches a model-visible tool result, so a
/// name containing a newline or a control character could reshape the text around it. Non-printable
/// characters are replaced rather than dropped, so the length and structure stay readable.
fn sanitize_pointer(pointer: &str) -> String {
    bounded_detail(&pointer.chars().map(sanitize_char).collect::<String>())
}

/// Keeps a keyword name to its safe characters.
///
/// A keyword name comes from the schema, which a connector authored, so it is bounded and filtered
/// for the same reason as the pointer.
fn sanitize_keyword(keyword: &str) -> String {
    let filtered: String = keyword.chars().map(sanitize_char).collect();
    bounded_detail(&filtered)
}

/// Replaces a control character with a middle dot.
fn sanitize_char(c: char) -> char {
    if c.is_control() { '·' } else { c }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The schema document every test below starts from.
    const OBJECT_SCHEMA: &str = r#"{
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": {
            "query": { "type": "string", "minLength": 1 }
        },
        "required": ["query"],
        "additionalProperties": false
    }"#;

    fn schema() -> ToolSchema {
        ToolSchema::parse(OBJECT_SCHEMA).unwrap_or_else(|error| panic!("valid schema: {error}"))
    }

    /// A well-formed 2020-12 schema is accepted and its document is available.
    #[test]
    fn a_valid_schema_is_accepted() {
        let schema = schema();
        assert_eq!(schema.document()["type"], "object");
        let encoded =
            serde_json::to_string(&schema.document()["required"]).unwrap_or_else(|_| String::new());
        assert_eq!(encoded, r#"["query"]"#);
    }

    /// A conforming instance validates, and a non-conforming one reports violations.
    #[test]
    fn instances_are_validated_with_violations_reported() {
        let schema = schema();
        let report = schema
            .validate(&json!({"query": "hello"}))
            .unwrap_or_else(|error| panic!("validate: {error}"));
        assert!(report.is_valid());

        let bad = schema
            .validate(&json!({}))
            .unwrap_or_else(|error| panic!("validate: {error}"));
        assert!(!bad.is_valid());
        assert!(
            bad.violations()
                .iter()
                .any(|violation| violation.keyword == "required"),
            "the report must name the keyword that rejected it: {:?}",
            bad.violations()
        );
    }

    /// A violation names a location, and never carries library prose.
    ///
    /// The point of the test is what is *absent*: the report crosses into a model-visible tool
    /// result, so it must be a pointer plus a keyword and not a message.
    #[test]
    fn a_violation_is_a_location_and_a_keyword() {
        let schema = schema();
        let report = schema
            .validate(&json!({"query": 5}))
            .unwrap_or_else(|error| panic!("validate: {error}"));
        let violation = report
            .violations()
            .first()
            .unwrap_or_else(|| panic!("the wrong type must be reported"));
        assert_eq!(violation.keyword, "type");
        assert_eq!(violation.instance_path, "/query");
        // A serialized violation has exactly two fields, which is what keeps prose out.
        let encoded = serde_json::to_value(violation).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            encoded.as_object().map(serde_json::Map::len),
            Some(2),
            "a violation carries a path and a keyword and nothing else"
        );
    }

    /// A document with no `$schema` is refused rather than assumed to be 2020-12.
    ///
    /// Defaulting would silently reinterpret an earlier draft: `exclusiveMinimum` is a boolean
    /// before 2020-12 and a number in it, so a correct older document would become an invalid new
    /// one and the author would be told their syntax was wrong.
    #[test]
    fn a_missing_dialect_is_refused() {
        let refused = ToolSchema::parse(r#"{"type":"object"}"#);
        assert_eq!(refused, Err(SchemaError::WrongDialect { found: None }));
    }

    /// A document declaring another dialect is refused, and the error names what it declared.
    #[test]
    fn a_wrong_dialect_is_refused_and_named() {
        let refused = ToolSchema::parse(
            r#"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object"}"#,
        );
        assert_eq!(
            refused,
            Err(SchemaError::WrongDialect {
                found: Some("http://json-schema.org/draft-07/schema#".to_owned())
            })
        );
    }

    /// Non-JSON and non-object documents are refused with distinct reasons.
    #[test]
    fn malformed_documents_are_refused_distinctly() {
        assert_eq!(ToolSchema::parse("not json"), Err(SchemaError::NotJson));
        assert_eq!(
            ToolSchema::parse(r#"["$schema"]"#),
            Err(SchemaError::NotAnObject)
        );
        assert_eq!(
            ToolSchema::parse(r#""text""#),
            Err(SchemaError::NotAnObject)
        );
    }

    /// **The security property: an external reference is refused rather than fetched.**
    ///
    /// With the crate's default features disabled there is no resolver at all, so a remote `$ref`
    /// could not be fetched even if this walk were removed — but it would then fail as an *invalid
    /// schema*, attributing a deliberate refusal to the author's syntax. This asserts the specific
    /// error, so the reason survives a future change to the feature set.
    #[test]
    fn an_external_reference_is_refused_by_name() {
        for reference in [
            "https://example.invalid/schema.json",
            "http://example.invalid/schema.json#/defs/x",
            "file:///etc/passwd",
            "./sibling.json",
            "../shared.json",
        ] {
            let document = format!(r#"{{"$schema":"{TOOL_SCHEMA_DIALECT}","$ref":"{reference}"}}"#);
            let refused = ToolSchema::parse(&document);
            assert_eq!(
                refused,
                Err(SchemaError::ExternalReference {
                    reference: reference.to_owned()
                }),
                "{reference} must be refused as an external reference"
            );
        }
    }

    /// A non-local `$id` is refused too, because it rebases later references.
    #[test]
    fn a_non_local_identifier_is_refused() {
        let document = format!(
            r#"{{"$schema":"{TOOL_SCHEMA_DIALECT}","$id":"https://example.invalid/root"}}"#
        );
        assert_eq!(
            ToolSchema::parse(&document),
            Err(SchemaError::ExternalReference {
                reference: "https://example.invalid/root".to_owned()
            })
        );
    }

    /// A local reference is accepted, so the refusal is not "any `$ref`".
    ///
    /// Without this test the previous one would pass for a walk that refused every reference, which
    /// would reject legitimate schemas composed with `$defs`.
    #[test]
    fn a_local_reference_is_accepted() {
        // `r##` rather than `r#`, because the JSON itself contains `"#` in the pointer fragment
        // and a single-hash raw string would end early at that point.
        let document = format!(
            r##"{{
                "$schema": "{TOOL_SCHEMA_DIALECT}",
                "$defs": {{ "query": {{ "type": "string", "minLength": 1 }} }},
                "type": "object",
                "properties": {{ "q": {{ "$ref": "#/$defs/query" }} }},
                "required": ["q"]
            }}"##
        );
        let schema =
            ToolSchema::parse(&document).unwrap_or_else(|error| panic!("local ref: {error}"));
        assert!(
            schema
                .validate(&json!({"q": "hello"}))
                .unwrap_or_else(|error| panic!("validate: {error}"))
                .is_valid()
        );
        // And it is still a real constraint: the referenced `minLength` applies.
        assert!(
            !schema
                .validate(&json!({"q": ""}))
                .unwrap_or_else(|error| panic!("validate: {error}"))
                .is_valid(),
            "the local reference must carry the constraint, not just parse"
        );
    }

    /// **A schema whose only keyword is unrecognized validates everything, which is why the
    /// metaschema check is not optional.**
    ///
    /// This is the defect the metaschema check exists to catch, stated as a test: `{"typo":
    /// "object"}` is accepted by the validator — JSON Schema ignores unknown keywords — so without
    /// the dialect and schema checks a tool could register an input contract that accepts anything.
    /// The dialect requirement is what refuses it here, because a document with an unknown keyword
    /// is *valid* 2020-12 and only the missing `$schema` catches it.
    #[test]
    fn an_unknown_keyword_alone_does_not_constrain_anything() {
        // With the dialect declared, this is a legally valid 2020-12 schema that accepts anything.
        let permissive = format!(r#"{{"$schema":"{TOOL_SCHEMA_DIALECT}","typo":"object"}}"#);
        let schema = ToolSchema::parse(&permissive)
            .unwrap_or_else(|error| panic!("valid but useless schema: {error}"));
        assert!(
            schema
                .validate(&json!("a string is accepted"))
                .unwrap_or_else(|error| panic!("validate: {error}"))
                .is_valid(),
            "an unknown keyword constrains nothing, which is why registration must check intent"
        );

        // Without the dialect it is refused outright, so the permissive case cannot be reached by
        // accident in a document that never declared a dialect at all.
        assert!(ToolSchema::parse(r#"{"typo":"object"}"#).is_err());
    }

    /// A genuinely invalid schema is refused with the author's error retained.
    #[test]
    fn an_invalid_schema_is_refused_with_a_detail() {
        let document = format!(r#"{{"$schema":"{TOOL_SCHEMA_DIALECT}","type": 42}}"#);
        match ToolSchema::parse(&document) {
            Err(SchemaError::InvalidSchema { detail }) => {
                assert!(!detail.is_empty(), "the detail must say something");
            }
            other => panic!("a non-string type must be an invalid schema, got {other:?}"),
        }
    }

    /// An oversized document is refused before it is parsed.
    #[test]
    fn an_oversized_schema_is_refused() {
        let padding = " ".repeat(MAX_TOOL_SCHEMA_BYTES + 1);
        assert_eq!(ToolSchema::parse(&padding), Err(SchemaError::TooLarge));
    }

    /// The reported violations are bounded, so a large invalid instance cannot produce unbounded
    /// output.
    ///
    /// The instance is an array of wrong-typed items rather than an object with many unknown
    /// properties: `additionalProperties: false` is evaluated once for the whole object and reports
    /// a single error listing the offenders, so an object-based premise would assert a bound that
    /// was never tested. Per-item keywords report one error per item, which is what exercises the
    /// limit.
    #[test]
    fn reported_violations_are_bounded() {
        let document = format!(
            r#"{{
                "$schema": "{TOOL_SCHEMA_DIALECT}",
                "type": "array",
                "items": {{ "type": "string" }}
            }}"#
        );
        let schema = ToolSchema::parse(&document).unwrap_or_else(|error| panic!("{error}"));

        let instance = serde_json::Value::Array(
            (0..(MAX_REPORTED_VIOLATIONS + 20))
                .map(serde_json::Value::from)
                .collect(),
        );
        let report = schema
            .validate(&instance)
            .unwrap_or_else(|error| panic!("validate: {error}"));
        assert_eq!(
            report.violations().len(),
            MAX_REPORTED_VIOLATIONS,
            "the report must be bounded"
        );
    }

    /// A hostile property name cannot reshape a model-visible report.
    ///
    /// The instance is caller-controlled, so a property name is attacker-influenced text that
    /// reaches a tool result. Control characters are replaced, so a name cannot introduce a newline
    /// and restructure whatever reads the pointer.
    #[test]
    fn a_hostile_property_name_is_sanitized() {
        let document = format!(
            r#"{{
                "$schema": "{TOOL_SCHEMA_DIALECT}",
                "type": "object",
                "required": ["injected"]
            }}"#
        );
        let schema = ToolSchema::parse(&document).unwrap_or_else(|error| panic!("{error}"));
        let report = schema
            .validate(&json!({"a\nb\u{7}": 1}))
            .unwrap_or_else(|error| panic!("validate: {error}"));
        for violation in report.violations() {
            assert!(
                !violation.instance_path.contains('\n'),
                "a newline must not survive into a report: {:?}",
                violation.instance_path
            );
            assert!(
                !violation.instance_path.chars().any(char::is_control),
                "no control character may survive: {:?}",
                violation.instance_path
            );
            assert!(!violation.keyword.chars().any(char::is_control));
        }
    }

    /// **Format is annotated, not asserted, and this is a deliberate decision stated as a test.**
    ///
    /// JSON Schema 2020-12 makes `format` annotation-only by default; asserting it is an opt-in. The
    /// opt-in is refused here because format checking is a **validation** that only covers the
    /// formats the library implements, and a tool that declared `"format": "email"` expecting a
    /// refusal would silently accept a non-address. The contract's answer is that a tool constrains
    /// shape with `pattern` — which is always checked — and treats a *reachability* claim about an
    /// address as something only the provider can establish. `docs/research/integrations/
    /// json-schema-validation.md` records this as the unresolved policy it was.
    #[test]
    fn format_is_annotated_and_not_asserted() {
        let document = format!(
            r#"{{
                "$schema": "{TOOL_SCHEMA_DIALECT}",
                "type": "string",
                "format": "email"
            }}"#
        );
        let schema = ToolSchema::parse(&document).unwrap_or_else(|error| panic!("{error}"));
        assert!(
            schema
                .validate(&json!("definitely not an address"))
                .unwrap_or_else(|error| panic!("validate: {error}"))
                .is_valid(),
            "format must not assert, so a tool cannot rely on it for enforcement"
        );

        // The keyword that IS always checked, so the refusal above is not "nothing is validated".
        let constrained = format!(
            r#"{{
                "$schema": "{TOOL_SCHEMA_DIALECT}",
                "type": "string",
                "pattern": "^[^@]+@[^@]+$"
            }}"#
        );
        let schema = ToolSchema::parse(&constrained).unwrap_or_else(|error| panic!("{error}"));
        assert!(
            !schema
                .validate(&json!("definitely not an address"))
                .unwrap_or_else(|error| panic!("validate: {error}"))
                .is_valid(),
            "pattern is checked, and is the contract's answer to format"
        );
    }

    /// Two schemas are equal when their documents are, which is what a registry keys on.
    #[test]
    fn equality_follows_the_document() {
        assert_eq!(schema(), schema());
        let other = ToolSchema::parse(&format!(
            r#"{{"$schema":"{TOOL_SCHEMA_DIALECT}","type":"string"}}"#
        ))
        .unwrap_or_else(|error| panic!("{error}"));
        assert_ne!(schema(), other);
    }

    /// The `Debug` form shows the schema, because a failing test should display the contract.
    #[test]
    fn debug_shows_the_document() {
        let rendered = format!("{:?}", schema());
        assert!(
            rendered.contains("query"),
            "debug output must show the schema"
        );
    }
}
