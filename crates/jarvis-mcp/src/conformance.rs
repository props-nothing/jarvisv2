//! MCP schema conformance: the `x-mcp-header` annotation and server-supplied schemas.
//!
//! # Why the client validates a server's annotation
//!
//! `x-mcp-header` lets a server mirror one tool argument into an HTTP header so that an intermediary
//! can route on it. The specification makes the **client** responsible for policing its own
//! constraints: a tool whose annotation violates them "**MUST** be excluded from the result of
//! `tools/list`". That is an unusual place for a validation to live, and the reason matters — the
//! constraint is about what the *client* can safely put in a header, so a server that gets it wrong
//! is a server whose tool this client cannot call correctly. Excluding one tool rather than failing
//! the list is the specification's explicit choice: a single malformed definition must not remove
//! every other tool.
//!
//! # Why the reachability rule is the interesting one
//!
//! Three of the four constraints are ordinary bounds. The reachability rule is not: an annotated
//! property must be reachable from the schema root through a chain of **`properties` keys only**,
//! never through `items`, `oneOf`/`anyOf`/`allOf`/`not`, `if`/`then`/`else`, or `$ref`. The
//! specification defines header extraction as reading the value at "the exact property path"
//! annotated, and that definition only has a single answer when the path is unique. Under `oneOf`
//! there are two answers; under `items` the path needs an array index the header cannot carry. So
//! the rule is not arbitrary strictness — it is what makes the extraction function well-defined, and
//! a client that skips the check cannot know which value to send.

use std::collections::BTreeMap;
use std::fmt;

use jarvis_tools::{ToolSchema, ToolSource};
use serde_json::{Map, Value};

/// The schema property that names the header a parameter is mirrored into.
pub const HEADER_ANNOTATION: &str = "x-mcp-header";

/// The `$ref` keyword, named because its presence ends a reachability chain.
const REF_KEYWORD: &str = "$ref";

/// Keywords whose presence between the root and an annotated property makes the path ambiguous.
///
/// Grouped rather than checked one at a time so the reason can name the family. A wildcard
/// (`patternProperties`, `additionalProperties`) belongs here for the same reason as `items`: the
/// path to a value under it is not a fixed string.
const TRAVERSAL_BLOCKERS: &[&str] = &[
    "items",
    "prefixItems",
    "contains",
    "oneOf",
    "anyOf",
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "$ref",
    "$dynamicRef",
    "patternProperties",
    "additionalProperties",
    "dependentSchemas",
    "unevaluatedItems",
    "unevaluatedProperties",
];

/// The primitive JSON Schema types an annotated property may have.
///
/// `number` is **absent deliberately**. The specification permits `integer`, `string`, and
/// `boolean`, and states plainly that parameters with type `number` are not permitted — a float
/// cannot round-trip through a header as text without a formatting decision the protocol has not
/// made.
const PERMITTED_HEADER_TYPES: &[&str] = &["integer", "string", "boolean"];

/// The inclusive bound on an annotated integer, which is JavaScript's safe range.
///
/// `2^53 - 1`, not `i64::MAX`, because the header value is compared numerically and the protocol
/// fixes the range at what a double can represent exactly. An integer beyond this could compare
/// equal to a different integer after a float round trip.
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Explains why a tool definition was excluded from discovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConformanceError {
    /// A parameter's schema is not an object, so it cannot carry an annotation at all.
    ParameterNotAnObject {
        /// The annotated parameter path.
        path: String,
        /// What the schema actually was.
        found: String,
    },
    /// The annotation value was empty or was not a string.
    InvalidHeaderName {
        /// The annotated parameter path.
        path: String,
        /// Why the value was refused.
        reason: String,
    },
    /// Two parameters named the same header, differing only in case.
    DuplicateHeaderName {
        /// The header name both derived, lowercased for the comparison.
        header: String,
        /// The first path that claimed it.
        first: String,
        /// The second path that claimed it.
        second: String,
    },
    /// The annotated property's type is not one a header may carry.
    DisallowedType {
        /// The annotated parameter path.
        path: String,
        /// What was declared, when anything was.
        declared: String,
    },
    /// The annotated property is not reachable from the root through `properties` alone.
    NotStaticallyReachable {
        /// The annotated parameter path.
        path: String,
        /// The keyword that made the path ambiguous.
        keyword: String,
    },
    /// A `$ref` was found anywhere in the schema.
    ReferenceNotSupported {
        /// Where the `$ref` was found.
        path: String,
        /// The reference's target, bounded.
        target: String,
    },
    /// The schema had to be parsed and could not be.
    UnreadableSchema {
        /// Why it could not be read.
        reason: String,
    },
}

impl fmt::Display for ConformanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParameterNotAnObject { path, found } => write!(
                formatter,
                "the parameter at {path} carries {HEADER_ANNOTATION} but its schema is {found}, not an object"
            ),
            Self::InvalidHeaderName { path, reason } => write!(
                formatter,
                "the parameter at {path} has an invalid {HEADER_ANNOTATION}: {reason}"
            ),
            Self::DuplicateHeaderName {
                header,
                first,
                second,
            } => write!(
                formatter,
                "the parameters at {first} and {second} both name the header {header}, \
                 which must be unique case-insensitively"
            ),
            Self::DisallowedType { path, declared } => write!(
                formatter,
                "the parameter at {path} carries {HEADER_ANNOTATION} but is typed {declared}, \
                 and only integer, string, and boolean may be mirrored into a header"
            ),
            Self::NotStaticallyReachable { path, keyword } => write!(
                formatter,
                "the parameter at {path} carries {HEADER_ANNOTATION} but is not reachable from the \
                 schema root through `properties` alone: it passes through `{keyword}`, which makes \
                 the property path ambiguous"
            ),
            Self::ReferenceNotSupported { path, target } => write!(
                formatter,
                "the schema references {target} at {path}, and JARVIS does not resolve `$ref` in a \
                 server-supplied schema"
            ),
            Self::UnreadableSchema { reason } => {
                write!(
                    formatter,
                    "the server-supplied schema could not be read: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for ConformanceError {}

/// How many problems are reported for one tool.
///
/// Bounded because the messages reach a log an operator reads, and because these strings derive from
/// third-party input: an unbounded list of them is an unbounded amount of a remote peer's text in a
/// report.
pub const MAX_CONFORMANCE_PROBLEMS: usize = 8;

/// The result of checking one server-supplied tool definition.
#[derive(Clone, Debug)]
pub struct ConformanceReport {
    /// The tool name as the server gave it.
    pub tool: String,
    /// Every problem found, bounded to [`MAX_CONFORMANCE_PROBLEMS`].
    pub problems: Vec<ConformanceError>,
    /// How many problems were found in total before bounding.
    pub total_problems: usize,
    /// The header names this tool mirrors, path → header, in path order.
    ///
    /// Reported only when the tool is conformant, because a partially-extracted set would be a set a
    /// caller could act on. Kept for the inventory so an operator can see which parameters leave
    /// JARVIS inside a header — which the specification warns about, since header values are visible
    /// to every intermediary on the path.
    pub mirrored: BTreeMap<String, String>,
}

impl ConformanceReport {
    /// Returns whether the tool may be offered.
    #[must_use]
    pub fn is_conformant(&self) -> bool {
        self.problems.is_empty()
    }

    /// Renders the problems as one bounded, reader-facing line.
    #[must_use]
    pub fn describe(&self) -> String {
        if self.problems.is_empty() {
            return String::new();
        }
        let mut rendered = self
            .problems
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        if self.total_problems > self.problems.len() {
            let extra = self.total_problems - self.problems.len();
            rendered.push_str("; and ");
            rendered.push_str(&extra.to_string());
            rendered.push_str(" more");
        }
        rendered
    }
}

/// Validates one server-supplied tool's input schema.
///
/// Returns a report rather than a `Result` because both outcomes are ordinary: "this tool is fine"
/// and "this tool must be excluded, and here is why". A `Result` would make the common case of one
/// bad tool in a list awkward to express and would invite failing the whole list.
///
/// Every check runs even after one has failed, so an operator fixing a schema sees all of its
/// problems at once rather than one per round trip.
#[must_use]
pub fn check_tool_schema(tool: &str, input_schema: &Value) -> ConformanceReport {
    let mut problems = Vec::new();
    let mut total = 0_usize;
    let mut mirrored = BTreeMap::new();

    if let Some(root) = input_schema.as_object() {
        let mut context = Walk {
            problems: &mut problems,
            total: &mut total,
            headers: &mut BTreeMap::new(),
            mirrored: &mut mirrored,
        };
        context.walk(root, "", true);
    } else {
        total += 1;
        problems.push(ConformanceError::UnreadableSchema {
            reason: "the schema is not a JSON object".to_owned(),
        });
    }

    let conformant = problems.is_empty();
    ConformanceReport {
        tool: tool.to_owned(),
        problems,
        total_problems: total,
        // A partially-validated tool reports no mirrored parameters: the set is only meaningful if
        // the whole tool passed, and offering a subset would let a caller believe it is complete.
        mirrored: if conformant {
            mirrored
        } else {
            BTreeMap::new()
        },
    }
}

/// Accumulates problems while walking a schema.
struct Walk<'a> {
    problems: &'a mut Vec<ConformanceError>,
    total: &'a mut usize,
    /// Header name (lowercased) → the parameter path that claimed it.
    headers: &'a mut BTreeMap<String, String>,
    mirrored: &'a mut BTreeMap<String, String>,
}

impl Walk<'_> {
    /// Records a problem, keeping at most [`MAX_CONFORMANCE_PROBLEMS`] and always counting it.
    fn record(&mut self, problem: ConformanceError) {
        *self.total += 1;
        if self.problems.len() < MAX_CONFORMANCE_PROBLEMS {
            self.problems.push(problem);
        }
    }

    /// Visits an object schema, recursing into `properties` and reporting ambiguous traversals.
    ///
    /// `reachable` is false once the walk has passed through a keyword that makes the path
    /// ambiguous, which is what turns the reachability rule into a property of the *walk* rather than
    /// of a single node — a node cannot know what it took to arrive.
    fn walk(&mut self, schema: &Map<String, Value>, path: &str, reachable: bool) {
        // A `$ref` is refused wherever it appears, reachable or not. `jarvis-tools` refuses a remote
        // reference already, and this revision made schemas more expressive, so the refusal is more
        // load-bearing rather than less: resolving one would mean fetching a URL a server chose.
        if let Some(Value::String(target)) = schema.get(REF_KEYWORD) {
            self.record(ConformanceError::ReferenceNotSupported {
                path: display_path(path),
                target: bound(target),
            });
        }

        if let Some(Value::Object(properties)) = schema.get("properties") {
            for (name, subschema) in properties {
                let child_path = format!("{path}/{name}");
                match subschema {
                    Value::Object(child) => self.visit_property(child, &child_path, reachable),
                    Value::Bool(_) => {
                        // A boolean subschema (`true`/`false`) cannot carry an annotation, so it has
                        // nothing to validate. It is legal JSON Schema and not an error.
                    }
                    other => {
                        if contains_annotation(other) {
                            self.record(ConformanceError::ParameterNotAnObject {
                                path: display_path(&child_path),
                                found: describe_json(other).to_owned(),
                            });
                        }
                    }
                }
            }
        }

        // Report every blocker that is present, so a schema with two problems names both.
        for keyword in TRAVERSAL_BLOCKERS {
            if schema.contains_key(*keyword) {
                // A blocker's *contents* may still hold annotated properties; those are the
                // unreachable ones, so they are reported with the keyword that made them so.
                self.report_unreachable_below(schema, keyword, path);
            }
        }
    }

    /// Checks one annotated-or-not property, following nested `properties` when present.
    fn visit_property(&mut self, schema: &Map<String, Value>, path: &str, reachable: bool) {
        if let Some(annotation) = schema.get(HEADER_ANNOTATION) {
            match annotation {
                Value::String(header) => {
                    // An unreachable path is reported once, at the blocker, which is the only place
                    // that knows which keyword made it ambiguous — so this arm is deliberately
                    // empty rather than reporting the annotation a second time here.
                    if reachable {
                        self.check_annotation(schema, path, header);
                    }
                }
                other => self.record(ConformanceError::InvalidHeaderName {
                    path: display_path(path),
                    reason: format!(
                        "the value must be a non-empty string, not {}",
                        describe_json(other)
                    ),
                }),
            }
        }
        self.walk(schema, path, reachable);
    }

    /// Applies the name, uniqueness, and type rules to one annotation.
    fn check_annotation(&mut self, schema: &Map<String, Value>, path: &str, header: &str) {
        let trimmed = header.trim();
        if trimmed.is_empty() {
            self.record(ConformanceError::InvalidHeaderName {
                path: display_path(path),
                reason: "the header name is empty".to_owned(),
            });
            return;
        }
        // HTTP token syntax: the characters an intermediary's parser accepts in a field name. The
        // specification requires this, and a name outside it is one a proxy may reject or mangle —
        // which would be a routing decision made on a value nobody validated.
        if let Some(character) = trimmed.chars().find(|c| !is_token_character(*c)) {
            self.record(ConformanceError::InvalidHeaderName {
                path: display_path(path),
                reason: format!(
                    "the header name contains {character:?}, which is not valid in an HTTP field name"
                ),
            });
            return;
        }

        let lowered = trimmed.to_ascii_lowercase();
        match self.headers.get(&lowered) {
            Some(first) if first != path => {
                self.record(ConformanceError::DuplicateHeaderName {
                    header: lowered,
                    first: display_path(first),
                    second: display_path(path),
                });
            }
            Some(_) => {}
            None => {
                self.headers.insert(lowered.clone(), path.to_owned());
            }
        }

        if !declares_permitted_type(schema) {
            self.record(ConformanceError::DisallowedType {
                path: display_path(path),
                declared: declared_type(schema),
            });
        }
        if let Some(out_of_range) = out_of_range_integer(schema) {
            self.record(ConformanceError::DisallowedType {
                path: display_path(path),
                declared: format!(
                    "integer bounded by {out_of_range}, which is outside the safe range"
                ),
            });
        }

        self.mirrored.insert(display_path(path), trimmed.to_owned());
    }

    /// Reports annotated properties that sit below a path-ambiguous keyword.
    fn report_unreachable_below(&mut self, schema: &Map<String, Value>, keyword: &str, path: &str) {
        let mut found = Vec::new();
        collect_annotations(schema.get(keyword), path, keyword, &mut found);
        if found.is_empty() {
            return;
        }
        for (annotation_path, blocker) in found {
            self.record(ConformanceError::NotStaticallyReachable {
                path: annotation_path,
                keyword: blocker,
            });
        }
    }
}

/// Returns whether an annotated property's declared types are all permitted.
///
/// A property with no `type` is accepted: the annotation is a client-side hint and an untyped schema
/// is not a violation the specification names. A property with a *list* of types is accepted only if
/// every member is permitted, because at runtime any of them could be the value.
fn declares_permitted_type(schema: &Map<String, Value>) -> bool {
    match schema.get("type") {
        Some(Value::String(single)) => PERMITTED_HEADER_TYPES.contains(&single.as_str()),
        Some(Value::Array(types)) => types.iter().all(|entry| {
            entry
                .as_str()
                .is_some_and(|name| PERMITTED_HEADER_TYPES.contains(&name))
        }),
        // No declared type, or a malformed one. Both are permitted: an untyped property is legal and
        // the protocol names no violation for it, and a *malformed* `type` is a schema-syntax problem
        // rather than a header problem — reporting it here would tell an operator the wrong thing.
        None | Some(_) => true,
    }
}

/// Renders the declared type for the error text.
fn declared_type(schema: &Map<String, Value>) -> String {
    match schema.get("type") {
        Some(Value::String(single)) => single.clone(),
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" or "),
        Some(other) => describe_json(other).to_owned(),
        None => "untyped".to_owned(),
    }
}

/// Returns a description of an `integer` bound that falls outside JavaScript's safe range.
///
/// `minimum`/`maximum` rather than `exclusiveMinimum`/`exclusiveMaximum` because a *bound* is what
/// has to fit: an exclusive bound of `2^53` admits only values at or below `2^53 - 1`, so it is
/// inside the range and must not be reported. Reporting it would refuse a legal schema, which is the
/// worse error for a check that excludes tools.
fn out_of_range_integer(schema: &Map<String, Value>) -> Option<&'static str> {
    let is_integer = match schema.get("type") {
        Some(Value::String(single)) => single == "integer",
        Some(Value::Array(types)) => types.iter().any(|t| t.as_str() == Some("integer")),
        _ => false,
    };
    if !is_integer {
        return None;
    }
    for (keyword, lower) in [("minimum", true), ("maximum", false)] {
        if let Some(Value::Number(number)) = schema.get(keyword)
            && let Some(value) = number.as_i64()
        {
            let outside = if lower {
                value < -MAX_SAFE_INTEGER
            } else {
                value > MAX_SAFE_INTEGER
            };
            if outside {
                return Some(if lower {
                    "a minimum below the safe range"
                } else {
                    "a maximum above the safe range"
                });
            }
        }
        // A float bound cannot be compared exactly, which is the reason `number` is disallowed in
        // the first place, so a non-integer bound is reported rather than approximated.
        if let Some(Value::Number(number)) = schema.get(keyword)
            && number.as_i64().is_none()
        {
            return Some("a non-integer bound on an integer");
        }
    }
    None
}

/// Returns whether a value anywhere in a subtree carries the header annotation.
fn contains_annotation(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key(HEADER_ANNOTATION) || map.values().any(contains_annotation)
        }
        Value::Array(items) => items.iter().any(contains_annotation),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

/// Collects annotated paths below a keyword, with the keyword that blocked each.
fn collect_annotations(
    value: Option<&Value>,
    path: &str,
    keyword: &str,
    found: &mut Vec<(String, String)>,
) {
    let Some(value) = value else {
        return;
    };
    match value {
        Value::Object(map) => {
            if map.contains_key(HEADER_ANNOTATION) {
                found.push((display_path(path), keyword.to_owned()));
            }
            for (name, child) in map {
                collect_annotations(Some(child), &format!("{path}/{name}"), keyword, found);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                collect_annotations(Some(child), &format!("{path}/{index}"), keyword, found);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Returns whether a character is valid in an HTTP field name.
///
/// RFC 9110's `tchar`. Written out rather than pulled from a crate: it is five lines, and the
/// specification names the exact grammar, so a dependency would only add a version to track.
fn is_token_character(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(
            character,
            '!' | '#'
                | '$'
                | '%'
                | '&'
                | '\''
                | '*'
                | '+'
                | '-'
                | '.'
                | '^'
                | '_'
                | '`'
                | '|'
                | '~'
        )
}

/// Renders a JSON pointer path, naming the root when the path is empty.
fn display_path(path: &str) -> String {
    if path.is_empty() {
        "(the schema root)".to_owned()
    } else {
        path.to_owned()
    }
}

/// Names a JSON value's shape for an error message.
fn describe_json(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Bounds third-party text for an error message, on a character boundary.
fn bound(value: &str) -> String {
    const LIMIT: usize = 120;
    let trimmed = value.trim();
    if trimmed.chars().count() <= LIMIT {
        return trimmed.to_owned();
    }
    trimmed.chars().take(LIMIT).collect()
}

/// Parses a server-supplied schema into a JARVIS [`ToolSchema`], refusing a reference.
///
/// Separate from [`check_tool_schema`] because the two answer different questions: this one asks
/// "can this become a JARVIS schema at all", and the conformance report asks "may this tool be
/// offered". A tool can be conformant about headers and still be unusable as a schema, and a caller
/// needs to tell those apart.
///
/// # Errors
///
/// Returns [`ConformanceError::ReferenceNotSupported`] when a `$ref` is present, and
/// [`ConformanceError::UnreadableSchema`] when `jarvis-tools` refuses the document — which includes
/// a remote `$ref`, a non-2020-12 dialect, and any schema that is not a valid schema object.
pub fn to_tool_schema(input_schema: &Value) -> Result<ToolSchema, ConformanceError> {
    if let Some(problem) = find_reference(input_schema, "") {
        return Err(problem);
    }
    ToolSchema::from_value(input_schema.clone()).map_err(|error| {
        ConformanceError::UnreadableSchema {
            reason: error.to_string(),
        }
    })
}

/// Finds the first `$ref` anywhere in a document, so the refusal can name its target.
///
/// Checked here as well as in the walk because the two callers differ: the walk refuses a reference
/// to *exclude a tool*, and this refuses one to *describe why the schema is unusable*. One of them
/// has to report the exact path, and doing it in one place with the same traversal is cheaper than
/// two implementations that could disagree.
fn find_reference(value: &Value, path: &str) -> Option<ConformanceError> {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(target)) = map.get(REF_KEYWORD) {
                return Some(ConformanceError::ReferenceNotSupported {
                    path: display_path(path),
                    target: bound(target),
                });
            }
            for (name, child) in map {
                if let Some(problem) = find_reference(child, &format!("{path}/{name}")) {
                    return Some(problem);
                }
            }
            None
        }
        Value::Array(items) => items
            .iter()
            .enumerate()
            .find_map(|(index, child)| find_reference(child, &format!("{path}/{index}"))),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    }
}

/// Describes a server-supplied schema for operator-facing inventory output.
///
/// Includes the source class so a reader cannot mistake a remote tool's schema for a native one:
/// `ToolSource::is_third_party` is the flag the rest of the system keys on, and this is where an
/// operator first meets it.
#[must_use]
pub fn describe_schema_source(source: ToolSource) -> &'static str {
    if source.is_third_party() {
        "third-party, untrusted"
    } else {
        "JARVIS-defined"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Builds a schema whose single property carries the annotation.
    ///
    /// By value because every call site builds the property inline, so there is nothing for the
    /// caller to keep; taking it by reference would add a `&` at each call site to work around a lint
    /// rather than to express anything.
    #[allow(clippy::needless_pass_by_value)]
    fn annotated(property: Value) -> Value {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": { "region": property },
            "additionalProperties": false
        })
    }

    fn conformant(tool: &str, schema: &Value) -> ConformanceReport {
        let report = check_tool_schema(tool, schema);
        assert!(
            report.is_conformant(),
            "{tool} should be conformant but reported: {}",
            report.describe()
        );
        report
    }

    fn rejected(tool: &str, schema: &Value) -> ConformanceReport {
        let report = check_tool_schema(tool, schema);
        assert!(!report.is_conformant(), "{tool} should have been excluded");
        report
    }

    #[test]
    fn a_well_formed_annotation_is_accepted_and_reported() {
        let schema = annotated(json!({ "type": "string", "x-mcp-header": "Region" }));
        let report = conformant("execute_sql", &schema);
        assert_eq!(
            report.mirrored.get("/region").map(String::as_str),
            Some("Region")
        );
        assert_eq!(report.total_problems, 0);
        assert!(report.describe().is_empty());
    }

    /// A schema with no annotation at all is the ordinary case and must not be touched.
    #[test]
    fn a_schema_without_annotations_is_conformant_and_mirrors_nothing() {
        let schema = json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        });
        let report = conformant("search", &schema);
        assert!(report.mirrored.is_empty());
    }

    /// The reason the rule exists: under `oneOf` the "exact property path" has more than one
    /// answer, so a client that skipped the check could not know which value to send.
    #[test]
    fn an_annotation_under_one_of_is_refused() {
        let schema = json!({
            "type": "object",
            "oneOf": [
                { "properties": { "region": { "type": "string", "x-mcp-header": "Region" } } },
                { "properties": { "zone": { "type": "string" } } }
            ]
        });
        let report = rejected("ambiguous", &schema);
        assert!(
            report.problems.iter().any(|problem| matches!(
                problem,
                ConformanceError::NotStaticallyReachable { keyword, .. } if keyword == "oneOf"
            )),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn an_annotation_under_items_is_refused() {
        let schema = json!({
            "type": "object",
            "properties": {
                "rows": {
                    "type": "array",
                    "items": { "properties": { "region": { "type": "string", "x-mcp-header": "Region" } } }
                }
            }
        });
        let report = rejected("arrayed", &schema);
        assert!(
            report.problems.iter().any(|problem| matches!(
                problem,
                ConformanceError::NotStaticallyReachable { keyword, .. } if keyword == "items"
            )),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn an_annotation_under_a_conditional_is_refused() {
        let schema = json!({
            "type": "object",
            "if": { "properties": { "kind": { "const": "a" } } },
            "then": { "properties": { "region": { "type": "string", "x-mcp-header": "R" } } }
        });
        let report = rejected("conditional", &schema);
        assert!(
            report.problems.iter().any(|problem| matches!(
                problem,
                ConformanceError::NotStaticallyReachable { keyword, .. } if keyword == "then"
            )),
            "{:?}",
            report.problems
        );
    }

    /// Nested objects ARE permitted as long as every step is a `properties` key, and this is the
    /// positive control for the reachability check: without it, a check that refused everything
    /// would pass every test above.
    #[test]
    fn an_annotation_reached_through_nested_properties_is_accepted() {
        let schema = json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "object",
                    "properties": {
                        "region": { "type": "string", "x-mcp-header": "Region" }
                    }
                }
            }
        });
        let report = conformant("nested", &schema);
        assert_eq!(
            report.mirrored.get("/location/region").map(String::as_str),
            Some("Region")
        );
    }

    /// `number` is explicitly not permitted, and an `integer` outside the double-safe range is not
    /// representable in a header comparison.
    #[test]
    fn a_number_and_an_unsafe_integer_are_both_refused() {
        let number = annotated(json!({ "type": "number", "x-mcp-header": "Amount" }));
        rejected("numbered", &number);

        let unsafe_integer = annotated(json!({
            "type": "integer",
            "minimum": -9_007_199_254_740_993_i64,
            "x-mcp-header": "Amount"
        }));
        rejected("unsafe", &unsafe_integer);

        // The boundary itself is inside the range and must be accepted from both sides, or the check
        // refuses a legal schema.
        let at_bound = annotated(json!({
            "type": "integer",
            "minimum": -MAX_SAFE_INTEGER,
            "maximum": MAX_SAFE_INTEGER,
            "x-mcp-header": "Amount"
        }));
        conformant("at-bound", &at_bound);

        // An exclusive bound of 2^53 admits only values within the range, so it is legal.
        let exclusive = annotated(json!({
            "type": "integer",
            "exclusiveMaximum": 9_007_199_254_740_992_u64,
            "x-mcp-header": "Amount"
        }));
        conformant("exclusive", &exclusive);
    }

    #[test]
    fn a_non_string_or_empty_annotation_is_refused() {
        for bad in [json!(7), json!(null), json!(["a"]), json!(""), json!("   ")] {
            let schema = annotated(json!({ "type": "string", "x-mcp-header": bad }));
            rejected("bad-annotation", &schema);
        }
    }

    #[test]
    fn a_header_name_outside_http_token_syntax_is_refused() {
        for bad in ["has space", "has:colon", "has\nnewline", "has,comma"] {
            let schema = annotated(json!({ "type": "string", "x-mcp-header": bad }));
            rejected("bad-name", &schema);
        }
    }

    /// The comparison must be case-insensitive, or two parameters could claim one header and only
    /// one value could be sent.
    #[test]
    fn header_names_colliding_only_by_case_are_refused() {
        let schema = json!({
            "type": "object",
            "properties": {
                "region": { "type": "string", "x-mcp-header": "Region" },
                "zone": { "type": "string", "x-mcp-header": "REGION" }
            }
        });
        let report = rejected("case-collision", &schema);
        assert!(
            report.problems.iter().any(|problem| matches!(
                problem,
                ConformanceError::DuplicateHeaderName { header, .. } if header == "region"
            )),
            "{:?}",
            report.problems
        );
    }

    #[test]
    fn a_dollar_ref_is_refused_wherever_it_appears() {
        let schema = json!({
            "type": "object",
            "properties": {
                "region": { "$ref": "https://example.com/schemas/region.json" }
            }
        });
        let report = rejected("referencing", &schema);
        assert!(
            report
                .problems
                .iter()
                .any(|problem| matches!(problem, ConformanceError::ReferenceNotSupported { .. })),
            "{:?}",
            report.problems
        );
        // And it is refused as a usable schema too, with the target named.
        let error = to_tool_schema(&schema)
            .err()
            .unwrap_or_else(|| panic!("a referencing schema must not convert"));
        assert!(error.to_string().contains("region.json"), "{error}");
    }

    /// **The specification's explicit choice**: one malformed tool must not remove the others, so
    /// the check reports per-tool and the caller excludes per-tool.
    #[test]
    fn a_malformed_tool_is_excluded_without_affecting_a_conformant_one() {
        let good = conformant(
            "good",
            &annotated(json!({ "type": "string", "x-mcp-header": "R" })),
        );
        let bad = rejected(
            "bad",
            &annotated(json!({ "type": "number", "x-mcp-header": "R" })),
        );
        assert!(good.is_conformant());
        assert!(!bad.is_conformant());
        // The good tool's mirrors survive the bad tool's failure.
        assert_eq!(good.mirrored.len(), 1);
        assert!(bad.mirrored.is_empty(), "a rejected tool mirrors nothing");
    }

    #[test]
    fn problems_are_bounded_and_the_total_is_reported() {
        // Many independent bad properties, so the bound is genuinely exceeded.
        let mut properties = Map::new();
        for index in 0..(MAX_CONFORMANCE_PROBLEMS + 5) {
            properties.insert(
                format!("p{index}"),
                json!({ "type": "number", "x-mcp-header": format!("H{index}") }),
            );
        }
        let schema = json!({ "type": "object", "properties": properties });
        let report = rejected("many", &schema);
        assert_eq!(report.problems.len(), MAX_CONFORMANCE_PROBLEMS);
        assert!(report.total_problems > MAX_CONFORMANCE_PROBLEMS);
        // The count must be visible, or a reader believes they saw every problem.
        assert!(report.describe().contains("more"), "{}", report.describe());
    }

    #[test]
    fn a_boolean_subschema_is_not_an_error() {
        let schema = json!({
            "type": "object",
            "properties": { "anything": true }
        });
        conformant("permissive", &schema);
    }

    #[test]
    fn a_non_object_schema_is_reported_rather_than_panicking() {
        for bad in [json!("text"), json!([]), json!(null), json!(3)] {
            let report = check_tool_schema("bad-root", &bad);
            assert!(!report.is_conformant());
            assert!(
                report
                    .problems
                    .iter()
                    .any(|problem| matches!(problem, ConformanceError::UnreadableSchema { .. }))
            );
        }
    }

    #[test]
    fn a_server_schema_converts_to_a_jarvis_schema() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"],
            "additionalProperties": false
        });
        assert!(to_tool_schema(&schema).is_ok());
    }

    /// The source description is what stops a reader mistaking a remote schema for a native one.
    #[test]
    fn the_source_description_distinguishes_third_party_tools() {
        assert_eq!(
            describe_schema_source(ToolSource::Mcp),
            "third-party, untrusted"
        );
        assert_eq!(describe_schema_source(ToolSource::Native), "JARVIS-defined");
        assert_eq!(
            describe_schema_source(ToolSource::Connector),
            "JARVIS-defined"
        );
    }
}
