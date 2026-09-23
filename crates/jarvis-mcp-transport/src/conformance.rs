//! Conformance evidence: this server's **emitted** results validated against the revision's official schema.
//!
//! # Why the authority is a vendored slice of the spec's schema and not the SDK
//!
//! `P3-010`'s whole point is that a client and a server built from the same library share their
//! assumptions, so a disagreement with the **specification** passes every test they write about each other.
//! `rmcp` is the library this server is built from, so a test asserting "the SDK serialized what we asked it
//! to" would be exactly that mistake — and the defect this module was written to catch **is an SDK default**.
//!
//! The authority is therefore the revision's machine-readable JSON Schema, fetched from the specification
//! repository, and the check is `jsonschema` — a crate with no relationship to MCP — validating a document
//! this server actually produced. Three layers, none of which shares an assumption with another: the
//! protocol's own schema, a general-purpose validator, and JARVIS's serialization.
//!
//! # What is vendored, and why it is a slice
//!
//! `tests/spec/` holds a **slice** of the official schema: the `$defs` reachable from the result types this
//! server emits. Vendoring the whole document would put ~181 KB of generated schema in the tree to validate
//! the answers to four methods, and the definitions that are not reachable cannot change a verdict. The slice
//! is derived, so `tests/spec/README.md` records the exact source URL, the access date, and the extraction
//! rule — and the extraction is a **reference-closure** computation rather than a hand-picked set, because a
//! hand-picked set is how a slice quietly stops covering the field that changes.
//!
//! The slice is Apache-2.0, like the specification it comes from, and no definition in it is modified:
//! `$ref`s inside it still point at `#/$defs/...`, which is why the closure must be complete for a reference
//! to resolve.

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::{Value, json};

    use crate::serve::{JarvisMcpServer, served_protocol_version};

    /// The vendored slice of the revision's schema.
    fn spec_schema() -> Value {
        let text = std::fs::read_to_string(spec_schema_path())
            .unwrap_or_else(|error| panic!("read the vendored spec schema: {error}"));
        serde_json::from_str(&text)
            .unwrap_or_else(|error| panic!("the vendored spec schema must be JSON: {error}"))
    }

    fn spec_schema_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("spec")
            .join("2026-07-28-result-slice.json")
    }

    /// Compiles a validator for one definition of the slice, with the slice's `$defs` beside it.
    ///
    /// `jsonschema` resolves `$ref` against the root document, so a definition lifted out of the slice needs
    /// its siblings carried along. That is the same technique `jarvis-tools`' `DocumentSet` uses for supplied
    /// documents, applied to a spec schema rather than to a tool's.
    fn validator_for_definition(definition: &str) -> jsonschema::Validator {
        let schema = spec_schema();
        let defs = schema
            .get("$defs")
            .unwrap_or_else(|| panic!("the vendored slice must carry $defs"));
        let target = defs
            .get(definition)
            .unwrap_or_else(|| panic!("the vendored slice must define {definition}"));
        let mut document = target.clone();
        document
            .as_object_mut()
            .unwrap_or_else(|| panic!("{definition} must be a schema object"))
            .insert("$defs".to_owned(), defs.clone());
        jsonschema::validator_for(&document)
            .unwrap_or_else(|error| panic!("compile {definition}: {error}"))
    }

    /// Asserts `instance` satisfies the named definition of the vendored slice.
    fn validate(definition: &str, instance: &Value) {
        let validator = validator_for_definition(definition);
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|error| format!("{} at {}", error, error.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "{definition} is not satisfied by the emitted document.\n\
             The protocol's schema is the authority here, not this crate's SDK.\nerrors: {errors:#?}\n\
             document: {}",
            serde_json::to_string_pretty(instance).unwrap_or_default()
        );
    }

    /// A server with one served tool, built the way the composition root builds one.
    fn server() -> JarvisMcpServer {
        crate::serve::tests::server_with_one_tool()
    }

    /// **`tools/list` conforms to the revision's schema, which is where the SDK's default does not.**
    ///
    /// This is `P3-010`'s discriminating test. `ListToolsResult` declares
    /// `"required": ["cacheScope", "resultType", "tools", "ttlMs"]` in `2026-07-28`, and
    /// `ListToolsResult::with_all_items` leaves two of the four unset — deliberately, for multi-era
    /// compatibility, which is not a reason that applies to a server advertising one era.
    ///
    /// **The document validated is the server's own**, taken by serializing what `tools_list_result` returns,
    /// which is the value `list_tools` hands the transport. The first version of this test assembled its own
    /// JSON from `tool_list()` plus the two constants; that version would have kept passing after the real
    /// construction changed, and it did. A test that restates the code it checks is not checking it.
    ///
    /// **Falsified by removing `.with_ttl_ms(..)` (or `.with_cache_scope(..)`) from `tools_list_result`:**
    /// the validator reports `"ttlMs" is a required property` and this test fails.
    #[test]
    fn a_tools_list_result_conforms_to_the_revision_schema() {
        let emitted = serde_json::to_value(server().tools_list_result())
            .unwrap_or_else(|error| panic!("serialize the server's tools/list result: {error}"));
        validate("ListToolsResult", &emitted);
    }

    /// **The slice rejects the document the SDK default would emit, so the test above is not vacuous.**
    ///
    /// A validation test is only evidence if the schema it validates against would **reject** the defect.
    /// Without this, a slice that accidentally lost its `required` list would make the test above pass for a
    /// non-conformant document, and nobody would learn that the check had stopped checking.
    ///
    /// The negative control omits exactly the two fields the SDK omits, which is the shape the defect takes,
    /// and includes a positive control so a broken fixture cannot masquerade as a detected defect.
    #[test]
    fn the_slice_rejects_the_document_the_sdk_default_would_emit() {
        let validator = validator_for_definition("ListToolsResult");

        // What `with_all_items` alone produces: no `ttlMs`, no `cacheScope`.
        let omitted = json!({ "resultType": "complete", "tools": [] });
        assert!(
            !validator.is_valid(&omitted),
            "the vendored slice must reject a document missing the required cache hints, or the \
             conformance test above proves nothing"
        );

        let complete = json!({
            "resultType": "complete",
            "tools": [],
            "ttlMs": 0,
            "cacheScope": "private",
        });
        assert!(
            validator.is_valid(&complete),
            "the slice must accept a conforming document, otherwise a failing validation would be \
             indistinguishable from a broken fixture"
        );
    }

    /// **Every `$defs` key the slice references is present in the slice.**
    ///
    /// The slice is extracted mechanically, so the failure this guards is a reference left dangling by an
    /// incomplete extraction — which `jsonschema` reports at compile time for the definition under test and
    /// would otherwise surface as a confusing error inside an unrelated test. Asserting closure here names
    /// the actual problem.
    #[test]
    fn the_slice_is_closed_under_reference() {
        let schema = spec_schema();
        let defs = schema
            .get("$defs")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("the vendored slice must carry $defs as an object"));
        let present: BTreeSet<&str> = defs.keys().map(String::as_str).collect();

        let text = serde_json::to_string(&schema)
            .unwrap_or_else(|error| panic!("serialize the vendored slice: {error}"));
        let mut missing = BTreeSet::new();
        let mut rest = text.as_str();
        while let Some(index) = rest.find("#/$defs/") {
            rest = &rest[index + "#/$defs/".len()..];
            let name: String = rest
                .chars()
                .take_while(|character| {
                    character.is_ascii_alphanumeric() || *character == '_' || *character == '-'
                })
                .collect();
            if !name.is_empty() && !present.contains(name.as_str()) {
                missing.insert(name);
            }
        }
        assert!(
            missing.is_empty(),
            "the vendored slice references definitions it does not carry, so extraction was incomplete: \
             {missing:#?}"
        );
    }

    /// **The slice is for the revision this server advertises, and that revision is one value.**
    ///
    /// Two facts this module leans on: the conformance verdict applies to `2026-07-28`, and the server claims
    /// exactly that. A server that started advertising several eras would make the vendored slice the wrong
    /// authority for some of its answers, and this test is where that shows up rather than in a reader's
    /// memory.
    #[test]
    fn the_slice_matches_the_revision_the_server_advertises() {
        assert_eq!(
            served_protocol_version(),
            "2026-07-28",
            "the vendored slice is for this revision; a different advertised revision needs its own slice"
        );
        let marker = spec_schema()
            .get("$comment")
            .and_then(Value::as_str)
            .map(str::to_owned);
        assert_eq!(
            marker,
            Some("derived slice of the MCP 2026-07-28 schema; see README.md".to_owned()),
            "the slice must carry its provenance marker, so a reader can tell what it is"
        );
    }
}
