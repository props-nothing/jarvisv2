//! Contract test for composing a tool schema against a JARVIS-supplied document set.
//!
//! This is the resolution `P3-001` deferred and stated as a cost: a schema that needs a shared
//! definition had no path but inlining. `P3-002` supplies one, and this test proves both halves of the
//! new rule rather than only the permissive one.
//!
//! **Both halves are required.** Asserting only that a supplied reference resolves would also pass if
//! `.offline()` and the disabled resolver features were doing nothing at all — the reference would
//! resolve because the library *fetched* it, and the SSRF surface `P3-001` closed would have silently
//! reopened. So the control case is asserted with the same schema shape: the same reference with
//! nothing supplied must be refused.

use jarvis_tools::{DocumentError, DocumentSet, SchemaError, TOOL_SCHEMA_DIALECT, ToolSchema};
use serde_json::json;

/// A shared definition, as JARVIS would ship one.
fn shared_document() -> serde_json::Value {
    json!({
        "$schema": TOOL_SCHEMA_DIALECT,
        "$id": "https://jarvis.invalid/shared/query.json",
        "type": "string",
        "minLength": 3,
        "maxLength": 40
    })
}

/// A tool schema that composes against the shared definition.
fn composing_schema() -> String {
    format!(
        r#"{{
            "$schema": "{TOOL_SCHEMA_DIALECT}",
            "type": "object",
            "properties": {{ "q": {{ "$ref": "https://jarvis.invalid/shared/query.json" }} }},
            "required": ["q"],
            "additionalProperties": false
        }}"#
    )
}

/// The documents a JARVIS-authored schema may compose against.
fn document_set() -> DocumentSet {
    let mut set = DocumentSet::new();
    set.insert(shared_document())
        .unwrap_or_else(|error| panic!("a well-formed shared document: {error}"));
    set
}

/// **A reference to a supplied document resolves, and its constraints are enforced.**
///
/// Enforcement, not just resolution: a `$ref` that resolved to an empty schema would satisfy a
/// "does it build" assertion while validating nothing.
#[test]
fn a_reference_to_a_supplied_document_resolves_and_is_enforced() {
    let schema = ToolSchema::parse_with(&composing_schema(), &document_set())
        .unwrap_or_else(|error| panic!("a supplied reference must resolve: {error}"));

    let valid = schema
        .validate(&json!({ "q": "hello" }))
        .unwrap_or_else(|error| panic!("validate: {error}"));
    assert!(valid.is_valid(), "a conforming instance must validate");

    // Too short: the supplied document's `minLength` applies.
    let short = schema
        .validate(&json!({ "q": "ab" }))
        .unwrap_or_else(|error| panic!("validate: {error}"));
    assert!(
        !short.is_valid(),
        "the supplied definition's minLength must be enforced, not merely resolved"
    );

    // Too long: `maxLength` from the same document applies, so the whole definition arrived.
    let long = schema
        .validate(&json!({ "q": "x".repeat(41) }))
        .unwrap_or_else(|error| panic!("validate: {error}"));
    assert!(!long.is_valid(), "maxLength must be enforced too");
}

/// **The control: the same reference with nothing supplied is refused.**
///
/// Without this, the test above would pass for a build that fetched the URL — which is the outcome
/// `P3-001` closed and this slice must not reopen.
#[test]
fn the_same_reference_without_a_supplied_document_is_refused() {
    let refused = ToolSchema::parse_with(&composing_schema(), &DocumentSet::new());
    assert_eq!(
        refused,
        Err(SchemaError::ExternalReference {
            reference: "https://jarvis.invalid/shared/query.json".to_owned()
        }),
        "an unsupplied reference must be refused by name"
    );

    // And the plain constructor behaves the same way, because it uses an empty set.
    assert_eq!(
        ToolSchema::parse(&composing_schema()),
        Err(SchemaError::ExternalReference {
            reference: "https://jarvis.invalid/shared/query.json".to_owned()
        })
    );
}

/// A URL that merely looks like a supplied identifier is still refused.
///
/// The rule is membership in the set, not a URL-shaped test. A schema referencing
/// `https://example.invalid/schema.json` is refused even when the set is non-empty and holds other
/// URLs, which is the difference between "may compose" and "may name any URL".
#[test]
fn an_unrelated_url_is_refused_even_with_a_non_empty_set() {
    let document = format!(
        r#"{{
            "$schema": "{TOOL_SCHEMA_DIALECT}",
            "type": "object",
            "properties": {{ "q": {{ "$ref": "https://example.invalid/schema.json" }} }}
        }}"#
    );
    assert_eq!(
        ToolSchema::parse_with(&document, &document_set()),
        Err(SchemaError::ExternalReference {
            reference: "https://example.invalid/schema.json".to_owned()
        })
    );
}

/// A local reference is still accepted with a set present, so the set did not replace the local rule.
#[test]
fn a_local_reference_is_still_accepted_alongside_a_set() {
    let document = format!(
        r##"{{
            "$schema": "{TOOL_SCHEMA_DIALECT}",
            "$defs": {{ "word": {{ "type": "string", "minLength": 2 }} }},
            "type": "object",
            "properties": {{ "w": {{ "$ref": "#/$defs/word" }} }},
            "required": ["w"]
        }}"##
    );
    let schema = ToolSchema::parse_with(&document, &document_set())
        .unwrap_or_else(|error| panic!("a local reference must still resolve: {error}"));
    assert!(
        schema
            .validate(&json!({ "w": "hi" }))
            .unwrap_or_else(|error| panic!("validate: {error}"))
            .is_valid()
    );
    assert!(
        !schema
            .validate(&json!({ "w": "h" }))
            .unwrap_or_else(|error| panic!("validate: {error}"))
            .is_valid()
    );
}

/// **A document that shadows a supplied `$id` cannot be added.**
///
/// The document-selection decision is the one a connector must not get influence over, so a second
/// declaration of one identifier is a refusal rather than a last-writer-wins insert.
#[test]
fn a_shadowing_document_cannot_be_added() {
    let mut set = document_set();
    let shadowing = json!({
        "$schema": TOOL_SCHEMA_DIALECT,
        "$id": "https://jarvis.invalid/shared/query.json",
        "type": "object"
    });
    assert_eq!(
        set.insert(shadowing),
        Err(DocumentError::DuplicateId {
            id: "https://jarvis.invalid/shared/query.json".to_owned()
        })
    );
    // And the original definition is still the one that resolves.
    let schema =
        ToolSchema::parse_with(&composing_schema(), &set).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        !schema
            .validate(&json!({ "q": 5 }))
            .unwrap_or_else(|error| panic!("validate: {error}"))
            .is_valid(),
        "the original string-typed definition must still apply"
    );
}

/// A schema referencing a **file** path is refused, because the walk does not distinguish local from
/// remote paths and a path is a filesystem primitive inside the tool path.
#[test]
fn a_file_reference_is_refused() {
    for reference in ["./sibling.json", "../shared.json", "file:///etc/passwd"] {
        let document = format!(r#"{{"$schema": "{TOOL_SCHEMA_DIALECT}","$ref":"{reference}"}}"#);
        assert_eq!(
            ToolSchema::parse_with(&document, &document_set()),
            Err(SchemaError::ExternalReference {
                reference: reference.to_owned()
            }),
            "{reference} must be refused"
        );
    }
}

/// A **manifest cannot compose**, because its `Deserialize` has no set to supply.
///
/// The asymmetry that keeps the document-selection decision out of external input: a stored
/// definition's schema is re-checked on load through `ToolSchema`'s `Deserialize`, and that path uses
/// an empty set. This asserts it, so a future change that threaded a set through deserialization
/// would fail here rather than silently granting manifests document access.
#[test]
fn a_deserialized_schema_cannot_compose_against_a_set() {
    let document: serde_json::Value =
        serde_json::from_str(&composing_schema()).unwrap_or_else(|error| panic!("{error}"));
    let decoded = serde_json::from_value::<ToolSchema>(document);
    assert!(
        decoded.is_err(),
        "a stored schema with an unresolved reference must not load"
    );

    // The same schema loads when it is self-contained, so the refusal is about the reference.
    let self_contained: serde_json::Value = serde_json::from_str(&format!(
        r#"{{
            "$schema": "{TOOL_SCHEMA_DIALECT}",
            "type": "object",
            "properties": {{ "q": {{ "type": "string" }} }},
            "required": ["q"]
        }}"#
    ))
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(serde_json::from_value::<ToolSchema>(self_contained).is_ok());
}
