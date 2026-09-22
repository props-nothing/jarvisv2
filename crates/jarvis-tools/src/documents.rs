//! The document set a tool schema may compose against.
//!
//! # Why this exists
//!
//! `P3-001` refused every non-local `$ref` and recorded the cost rather than hiding it: a schema that
//! legitimately needs a shared definition had no path but inlining. This module is that path, and the
//! restriction is now precise instead of total: a reference resolves **only** against a document
//! JARVIS itself was given, and nothing is ever fetched.
//!
//! Verified rather than assumed (`crates/jarvis-tools/tests/supplied_documents.rs`): with the
//! resolver's default features disabled **and** `.offline()` set, a reference to a document in this
//! set resolves and the definition's constraints are enforced. The control — the same reference with
//! nothing supplied — is refused. Both halves are asserted, because the first alone would also pass
//! if `offline()` were doing nothing at all.
//!
//! # Why an external manifest still may not use one
//!
//! A connector manifest is external input, and it is deserialized through
//! [`crate::ToolSchema`]'s `Deserialize`, which has no document set and therefore refuses any
//! non-local reference. That asymmetry is deliberate and is not a gap:
//!
//! - **JARVIS-authored schemas may compose.** The documents come from JARVIS's own build, so the set
//!   is closed over content this project wrote and reviewed.
//! - **A connector manifest must be self-contained.** If a manifest could name a document, its author
//!   would gain influence over *which* JARVIS document is loaded — and `$id` is a document-controlled
//!   string, so a manifest declaring an `$id` matching a JARVIS URI could shadow the definition it
//!   claims to reference. Requiring the manifest to inline the definition makes the manifest's
//!   content the only thing the manifest controls, which is the property that keeps the tool path
//!   free of a document-selection decision.
//!
//! The cost is stated rather than hidden: a connector with a large shared schema must repeat it. That
//! is a size cost paid at authoring time, against a correctness property paid every time a tool is
//! validated.

use std::collections::BTreeMap;

use serde_json::Value;
use thiserror::Error;

use crate::schema::{MAX_TOOL_SCHEMA_BYTES, TOOL_SCHEMA_DIALECT};

/// Maximum documents in one set.
///
/// Bounded for the same reason a registry is: a document set is supplied by a caller and each entry is
/// compiled into every schema that references it.
pub const MAX_SUPPLIED_DOCUMENTS: usize = 64;

/// Explains why a supplied document was rejected.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DocumentError {
    /// The document was not a JSON object.
    #[error("a supplied schema document must be a JSON object")]
    NotAnObject,
    /// The document exceeded the per-document size bound.
    #[error("the supplied schema document exceeds {MAX_TOOL_SCHEMA_BYTES} bytes")]
    TooLarge,
    /// The document did not declare the required dialect.
    #[error("a supplied schema document must declare $schema = {TOOL_SCHEMA_DIALECT}")]
    WrongDialect,
    /// The document declared no `$id`, so nothing could reference it.
    #[error("a supplied schema document must declare an $id to be referenceable")]
    MissingId,
    /// Two documents in one set declared the same `$id`.
    ///
    /// A **second** declaration of one identifier is the shadowing case this module exists to prevent:
    /// which definition a reference resolved to would depend on insertion order, so the same schema
    /// could validate differently in two builds.
    #[error("the $id {id} is declared by more than one supplied document")]
    DuplicateId {
        /// The identifier declared twice.
        id: String,
    },
    /// The set exceeded [`MAX_SUPPLIED_DOCUMENTS`].
    #[error("a document set may hold at most {MAX_SUPPLIED_DOCUMENTS} documents")]
    TooManyDocuments,
}

/// A closed set of documents a schema may reference by `$id`.
///
/// Keyed by `$id` in a `BTreeMap` so iteration and error reporting are in stable order.
#[derive(Clone, Debug, Default)]
pub struct DocumentSet {
    documents: BTreeMap<String, Value>,
}

impl DocumentSet {
    /// Creates an empty set, which refuses every non-local reference.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a document under the `$id` it declares.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError`] when the document is not an object, is oversized, declares another
    /// dialect, declares no `$id`, duplicates an existing `$id`, or the set is full.
    ///
    /// The `$id` is read from the document rather than passed in, so a caller cannot insert a document
    /// under a name it does not declare. That matters because a mismatch would make the reference that
    /// resolves and the identifier in the document disagree — the reference would succeed while
    /// resolving to something other than what its target claims to be.
    pub fn insert(&mut self, document: Value) -> Result<(), DocumentError> {
        let Some(object) = document.as_object() else {
            return Err(DocumentError::NotAnObject);
        };

        let encoded_len = serde_json::to_string(&document).map_or(usize::MAX, |text| text.len());
        if encoded_len > MAX_TOOL_SCHEMA_BYTES {
            return Err(DocumentError::TooLarge);
        }

        let declared = object
            .get("$schema")
            .and_then(Value::as_str)
            .is_some_and(|dialect| dialect == TOOL_SCHEMA_DIALECT);
        if !declared {
            return Err(DocumentError::WrongDialect);
        }

        let Some(id) = object.get("$id").and_then(Value::as_str) else {
            return Err(DocumentError::MissingId);
        };
        let id = id.to_owned();

        if self.documents.contains_key(&id) {
            return Err(DocumentError::DuplicateId { id });
        }
        if self.documents.len() >= MAX_SUPPLIED_DOCUMENTS {
            return Err(DocumentError::TooManyDocuments);
        }

        self.documents.insert(id, document);
        Ok(())
    }

    /// Returns whether a reference could resolve within this set.
    ///
    /// Used by [`crate::ToolSchema`] to decide which references to refuse **before** the validator is
    /// built, so an unresolvable reference is reported as a specific refusal rather than as a generic
    /// invalid-schema error.
    #[must_use]
    pub fn resolves(&self, reference: &str) -> bool {
        self.documents.contains_key(reference)
    }

    /// Returns how many documents the set holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// Returns whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// Builds the resolver `jsonschema` needs from this set.
    ///
    /// # Errors
    ///
    /// Returns a message when a document's `$id` is not a usable URI, which `referencing` reports at
    /// build time. Returned as a `String` because it is an author error surfaced at registration, in
    /// the same spirit as [`crate::SchemaError::InvalidSchema`]'s detail.
    pub(crate) fn build_registry(&self) -> Result<jsonschema::Registry<'static>, String> {
        let mut builder = jsonschema::Registry::new();
        for (id, document) in &self.documents {
            let resource = jsonschema::Resource::from_contents(document.clone());
            builder = builder
                .add(id.clone(), resource)
                .map_err(|error| error.to_string())?;
        }
        builder.prepare().map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document(id: &str) -> Value {
        json!({
            "$schema": TOOL_SCHEMA_DIALECT,
            "$id": id,
            "type": "string",
            "minLength": 1
        })
    }

    /// A well-formed document is accepted and becomes resolvable by its own `$id`.
    #[test]
    fn a_document_is_accepted_under_its_own_id() {
        let mut set = DocumentSet::new();
        assert!(set.is_empty());
        set.insert(document("https://jarvis.invalid/shared.json"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(set.len(), 1);
        assert!(set.resolves("https://jarvis.invalid/shared.json"));
        assert!(!set.resolves("https://jarvis.invalid/other.json"));
    }

    /// Malformed documents are refused with distinct reasons.
    #[test]
    fn malformed_documents_are_refused_distinctly() {
        let mut set = DocumentSet::new();
        assert_eq!(
            set.insert(json!(["not", "an", "object"])),
            Err(DocumentError::NotAnObject)
        );
        // A dialect but no `$id`: nothing could reference it, so it is not a supplied document.
        assert_eq!(
            set.insert(json!({ "$schema": TOOL_SCHEMA_DIALECT, "type": "string" })),
            Err(DocumentError::MissingId)
        );
        // An `$id` but no dialect: it would be interpreted under an unknown draft.
        assert_eq!(
            set.insert(json!({ "$id": "https://jarvis.invalid/x.json", "type": "string" })),
            Err(DocumentError::WrongDialect)
        );
        assert!(
            set.is_empty(),
            "a refused document must not be stored under any name"
        );
    }

    /// **A second document declaring one `$id` is refused, so a reference cannot be shadowed.**
    ///
    /// Which definition a reference resolved to would otherwise depend on insertion order, so the same
    /// schema could validate differently in two builds.
    #[test]
    fn a_duplicate_identifier_is_refused() {
        let mut set = DocumentSet::new();
        set.insert(document("https://jarvis.invalid/shared.json"))
            .unwrap_or_else(|error| panic!("{error}"));

        let shadowing = json!({
            "$schema": TOOL_SCHEMA_DIALECT,
            "$id": "https://jarvis.invalid/shared.json",
            "type": "object"
        });
        assert_eq!(
            set.insert(shadowing),
            Err(DocumentError::DuplicateId {
                id: "https://jarvis.invalid/shared.json".to_owned()
            })
        );
        assert_eq!(set.len(), 1, "the first definition must survive");
    }

    /// The `$id` is read from the document rather than supplied, so a document cannot be filed under a
    /// name it does not declare.
    #[test]
    fn a_document_is_filed_under_the_id_it_declares() {
        let mut set = DocumentSet::new();
        set.insert(document("https://jarvis.invalid/declared.json"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(set.resolves("https://jarvis.invalid/declared.json"));
        // There is no way to file it under anything else, so this is the only usable name.
        assert!(!set.resolves("https://jarvis.invalid/undeclared.json"));
    }

    /// An oversized document is refused.
    #[test]
    fn an_oversized_document_is_refused() {
        let mut set = DocumentSet::new();
        let huge = json!({
            "$schema": TOOL_SCHEMA_DIALECT,
            "$id": "https://jarvis.invalid/huge.json",
            "description": "x".repeat(MAX_TOOL_SCHEMA_BYTES)
        });
        assert_eq!(set.insert(huge), Err(DocumentError::TooLarge));
    }

    /// The set is bounded.
    #[test]
    fn the_set_is_bounded() {
        let mut set = DocumentSet::new();
        for index in 0..MAX_SUPPLIED_DOCUMENTS {
            set.insert(document(&format!("https://jarvis.invalid/doc{index}.json")))
                .unwrap_or_else(|error| panic!("doc{index}: {error}"));
        }
        assert_eq!(set.len(), MAX_SUPPLIED_DOCUMENTS);
        assert_eq!(
            set.insert(document("https://jarvis.invalid/overflow.json")),
            Err(DocumentError::TooManyDocuments)
        );
        assert_eq!(set.len(), MAX_SUPPLIED_DOCUMENTS);
    }

    /// An empty set resolves nothing, which is what makes the refusal total by default.
    #[test]
    fn an_empty_set_resolves_nothing() {
        let set = DocumentSet::new();
        assert!(!set.resolves("https://jarvis.invalid/shared.json"));
        assert!(set.is_empty());
    }

    /// The resolver is built without a retriever, so nothing in it can fetch.
    #[test]
    fn a_built_resolver_accepts_the_supplied_documents() {
        let mut set = DocumentSet::new();
        set.insert(document("https://jarvis.invalid/shared.json"))
            .unwrap_or_else(|error| panic!("{error}"));
        let registry = set
            .build_registry()
            .unwrap_or_else(|error| panic!("registry: {error}"));
        assert!(registry.contains_resource("https://jarvis.invalid/shared.json"));
        assert!(!registry.contains_resource("https://jarvis.invalid/absent.json"));
    }
}
