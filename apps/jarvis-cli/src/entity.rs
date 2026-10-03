//! The `jarvis entity` verbs (`P4-016`).
//!
//! # Why this is its own verb group rather than part of `jarvis memory`
//!
//! An entity is not a memory. It is the **subject** a memory is about, and the two have different lifecycles:
//! a memory is proposed, confirmed, corrected, and forgotten, while an entity is created, aliased, and merged.
//! Putting these verbs under `memory` would imply an entity is a kind of memory and would put "create the
//! subject" behind the verb that requires one — which is exactly the ordering problem `P4-008` recorded.
//!
//! # Why `lookup` is one verb and not two
//!
//! `jarvis entity lookup --label X` and `jarvis entity lookup --alias-kind K --alias-value V` are one question
//! — "which entity does this name denote" — answered by the same daemon route. Two verbs would be two places to
//! keep the same rendering, and the rendering is where the surface must be careful: a lookup that printed one
//! match would present a candidate as an identity, which is the rule the whole surface exists to respect.
//!
//! # The verbs are thin on purpose
//!
//! Each turns arguments into a request and renders a reply. No verb decides whether an alias is verified, how a
//! merge resolves, or which match is the right one — those are the daemon's and the user's, and a client that
//! re-derived them would be a second implementation of a rule that already has one home.

use crate::api_client::{ApiClient, ApiError};
use crate::output::ExitStatus;
use jarvis_protocol::{
    AddAliasRequest, CreateEntityRequest, EntityAliasReply, EntityDetailReply, EntityListReply,
    EntityLookupReply, EntityReply, MergeEntityRequest,
};
// Imported at module scope rather than inside the test module, because the `candidate` helper below is a
// `#[cfg(test)]` **function** in this module and a type in its signature resolves here rather than in the
// module that calls it.
#[cfg(test)]
use jarvis_protocol::EntityMatchReply;

/// Maximum entities one listing asks for.
///
/// Sent explicitly rather than left absent, so the page a user sees is the page the client asked for. The
/// daemon's own bound is the authority; this makes the client's expectation visible in the request.
const DEFAULT_PAGE: u32 = 50;

/// Runs one entity verb.
pub async fn run(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let json = crate::json_requested(arguments);
    match arguments.get(1).map(String::as_str) {
        Some("list") | None => list(client, json).await,
        Some("show") => show(client, arguments, json).await,
        Some("create") => create(client, arguments, json).await,
        Some("lookup") => lookup(client, arguments, json).await,
        Some("alias") => alias(client, arguments, json).await,
        Some("merge") => merge(client, arguments, json).await,
        Some(other) => {
            eprintln!("jarvis: unknown entity command {other:?}");
            eprintln!("{}", usage());
            ExitStatus::Usage
        }
    }
}

/// `jarvis entity [list]`
async fn list(client: &ApiClient, json: bool) -> ExitStatus {
    match client.list_entities(Some(DEFAULT_PAGE)).await {
        Ok(reply) => {
            render_list(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("list", &error),
    }
}

/// `jarvis entity show <id>`
async fn show(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(entity_id) = positional(arguments, 2) else {
        eprintln!("jarvis: entity show requires an entity identifier");
        return ExitStatus::Usage;
    };
    match client.read_entity(&entity_id).await {
        Ok(reply) => {
            render_detail(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("show", &error),
    }
}

/// `jarvis entity create <label> --kind K --confidence C [--attributes JSON]`
///
/// # Why `--kind` and `--confidence` are required
///
/// The same reasoning `jarvis memory remember` records for its own two flags. A kind decides what the entity
/// is — a person and a project are not interchangeable — and a confidence is a statement the user is making,
/// which the daemon deliberately refuses to guess. A default for either would store a fact the user did not
/// state, and the wrong pair is not detectable: an `organization` recorded as a `person` is perfectly storable
/// and simply wrong.
async fn create(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(label) = positional(arguments, 2) else {
        eprintln!("jarvis: entity create requires a label");
        eprintln!("{}", usage());
        return ExitStatus::Usage;
    };
    let Some(kind) = flag_value(arguments, "--kind") else {
        eprintln!("jarvis: entity create requires --kind (for example: --kind person)");
        return ExitStatus::Usage;
    };
    let Some(confidence) = flag_value(arguments, "--confidence") else {
        eprintln!(
            "jarvis: entity create requires --confidence (unverified, uncertain, likely, confirmed)"
        );
        return ExitStatus::Usage;
    };
    let request = CreateEntityRequest {
        label,
        kind,
        confidence,
        attributes: flag_value(arguments, "--attributes"),
    };
    match client.create_entity(&request).await {
        Ok(reply) => {
            // The identifier goes to **stderr** so `--json` off stays pipeable: the identifier is what the user
            // needs next (to remember or summarize about this entity), and a script capturing stdout must not
            // have it interleaved.
            eprintln!("entity {}", reply.entity.entity_id);
            render_detail(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("create", &error),
    }
}

/// `jarvis entity lookup --label L` or `--alias-kind K --alias-value V`
async fn lookup(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let label = flag_value(arguments, "--label");
    let alias_kind = flag_value(arguments, "--alias-kind");
    let alias_value = flag_value(arguments, "--alias-value");
    let result = match (label, alias_kind, alias_value) {
        (Some(label), _, _) => client.lookup_entity_by_label(&label).await,
        (None, Some(kind), Some(value)) => client.lookup_entity_by_alias(&kind, &value).await,
        (None, Some(_), None) => {
            eprintln!("jarvis: --alias-kind also needs --alias-value");
            return ExitStatus::Usage;
        }
        (None, None, Some(_)) => {
            eprintln!("jarvis: --alias-value also needs --alias-kind");
            return ExitStatus::Usage;
        }
        (None, None, None) => {
            eprintln!("jarvis: entity lookup requires --label, or --alias-kind with --alias-value");
            eprintln!("{}", usage());
            return ExitStatus::Usage;
        }
    };
    match result {
        Ok(reply) => {
            render_lookup(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("lookup", &error),
    }
}

/// `jarvis entity alias <id> --kind K --value V --verification S --source SRC --confidence C`
///
/// Every field is required, and that is the schema's shape rather than a CLI preference: a `confirmed` alias
/// must come from a user statement, and a `probabilistic` one cannot be held as confirmed. Defaulting any of the
/// three would let the client compose a combination the user did not choose, and the daemon would refuse it —
/// so the refusal would name a rule the user never saw.
async fn alias(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(entity_id) = positional(arguments, 2) else {
        eprintln!("jarvis: entity alias requires an entity identifier");
        return ExitStatus::Usage;
    };
    let required = [
        (
            "--kind",
            "the alias kind (name, email, handle, phone, provider_id, url, other)",
        ),
        ("--value", "the alias value"),
        (
            "--verification",
            "how it was established (confirmed, provider_id, exact_identifier, probabilistic)",
        ),
        (
            "--source",
            "where it came from (user_statement, user_correction, provider_record, tool_observation, document, model_inference, external_content)",
        ),
        (
            "--confidence",
            "how confidently it matches (unverified, uncertain, likely, confirmed)",
        ),
    ];
    let mut values = Vec::with_capacity(required.len());
    for (flag, description) in required {
        // `let .. else` rather than `match` on one arm: the pattern is a single case with an early return, and
        // the repository's lint set is what keeps the two shapes from being mixed across call sites.
        let Some(value) = flag_value(arguments, flag) else {
            eprintln!("jarvis: entity alias requires {flag} — {description}");
            return ExitStatus::Usage;
        };
        values.push(value);
    }
    let request = AddAliasRequest {
        alias_kind: values[0].clone(),
        alias_value: values[1].clone(),
        verification: values[2].clone(),
        source_kind: values[3].clone(),
        confidence: values[4].clone(),
    };
    match client.add_entity_alias(&entity_id, &request).await {
        Ok(reply) => {
            render_detail(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("alias", &error),
    }
}

/// `jarvis entity merge <source-id> --into <target-id>`
///
/// # Why the direction is spelled `--into` rather than a positional
///
/// A merge is the one operation here whose arguments are not interchangeable, and the pair an operator is most
/// likely to reverse. `--into` states it in words: the positional is merged **away**, the flag is merged
/// **into**. Both identifiers are printed in the success line so a reversal is visible in the receipt rather
/// than only in the database.
async fn merge(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(source_id) = positional(arguments, 2) else {
        eprintln!("jarvis: entity merge requires the identifier being merged away");
        return ExitStatus::Usage;
    };
    let Some(target_id) = flag_value(arguments, "--into") else {
        eprintln!("jarvis: entity merge requires --into <target-id>, the entity kept");
        eprintln!("{}", usage());
        return ExitStatus::Usage;
    };
    if source_id == target_id {
        eprintln!("jarvis: an entity cannot be merged into itself");
        return ExitStatus::Usage;
    }
    let request = MergeEntityRequest { target_id };
    match client.merge_entity(&source_id, &request).await {
        Ok(reply) => {
            eprintln!("merged {source_id} into {}", reply.entity.entity_id);
            render_detail(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("merge", &error),
    }
}

fn render_list(reply: &EntityListReply, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(reply).unwrap_or_default()
        );
        return;
    }
    if reply.entities.is_empty() {
        println!("no entities");
        return;
    }
    for entity in &reply.entities {
        println!("{}", entity_line(entity));
    }
    println!("{} of at most {}", reply.returned, reply.limit);
}

fn render_detail(reply: &EntityDetailReply, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(reply).unwrap_or_default()
        );
        return;
    }
    println!("{}", entity_line(&reply.entity));
    if let Some(merged) = reply.entity.merged_into.as_deref() {
        println!("  merged into {merged}");
    }
    if reply.aliases.is_empty() {
        println!("  no aliases");
    }
    for alias in &reply.aliases {
        println!("  alias {}", alias_line(alias));
    }
}

/// Renders a lookup, **listing every candidate** and never one.
///
/// This is the rendering the surface's rule lives in. A lookup that printed `matches[0]` would present a
/// candidate as the answer, and the daemon's whole reason for returning a list is that the architecture's rule
/// is "ambiguous aliases remain separate candidates" — so a client that picked one would undo it at the last
/// step. A single match is annotated as verified or not, because that is the difference between "this name
/// denotes this entity" and "this is the only guess".
fn render_lookup(reply: &EntityLookupReply, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(reply).unwrap_or_default()
        );
        return;
    }
    if reply.matches.is_empty() {
        println!("no match");
        return;
    }
    for candidate in &reply.matches {
        let verdict = if candidate.verified {
            "verified"
        } else {
            "candidate"
        };
        println!("{verdict} {}", entity_line(&candidate.entity));
        for alias in &candidate.matched_aliases {
            println!("  matched alias {}", alias_line(alias));
        }
    }
    if reply.returned > 1 {
        // Said out loud, because two matches mean the caller has to choose and nothing about the list says so.
        println!(
            "{} candidates — disambiguate before using one",
            reply.returned
        );
    }
}

fn entity_line(entity: &EntityReply) -> String {
    format!(
        "{} [{}] {} ({}, {} claims)",
        entity.entity_id, entity.kind, entity.label, entity.confidence, entity.linked_memories
    )
}

fn alias_line(alias: &EntityAliasReply) -> String {
    format!(
        "{}={} [{} {}]",
        alias.alias_kind, alias.alias_value, alias.verification, alias.confidence
    )
}

/// Reports a failed verb, with the exit status the failure deserves.
///
/// The same three-way split `jarvis memory` makes, and for the same reason: a **refusal** is the user's to act
/// on (`Denied`), while a transport or decode failure is the environment's (`Unavailable`). Collapsing them
/// would make a script unable to tell "your request was wrong" from "the daemon is down", which are the two
/// cases a caller retries differently.
fn report(operation: &str, error: &ApiError) -> ExitStatus {
    eprintln!("jarvis: entity {operation} failed: {error}");
    match error {
        ApiError::Refused(_) | ApiError::Identifier(_) => ExitStatus::Denied,
        ApiError::Transport(_) | ApiError::UnexpectedStatus { .. } | ApiError::Decode => {
            ExitStatus::Unavailable
        }
    }
}

/// Returns the argument at `index` when it is not a flag.
fn positional(arguments: &[String], index: usize) -> Option<String> {
    arguments
        .get(index)
        .filter(|value| !value.starts_with("--"))
        .cloned()
}

/// Returns the value following `flag`, when both are present.
///
/// A flag whose next argument is another flag is reported as absent rather than taking the flag as its value:
/// `--into --json` would otherwise merge into an entity literally named `--json`, which fails with "not found"
/// and sends the operator looking for a missing row instead of a missing argument.
fn flag_value(arguments: &[String], flag: &str) -> Option<String> {
    let index = arguments.iter().position(|value| value == flag)?;
    arguments
        .get(index + 1)
        .filter(|value| !value.starts_with("--"))
        .cloned()
}

fn usage() -> String {
    "usage: jarvis entity <list|show|create|lookup|alias|merge> [...]\n       \
     jarvis entity show <id>\n       \
     jarvis entity create <label> --kind K --confidence C [--attributes JSON]\n       \
     jarvis entity lookup --label L | --alias-kind K --alias-value V\n       \
     jarvis entity alias <id> --kind K --value V --verification S --source SRC --confidence C\n       \
     jarvis entity merge <source-id> --into <target-id>"
        .to_owned()
}

/// A match reply with no aliases, for a renderer test that needs one.
///
/// A lookup's rendering is the surface's rule — never print one match as the answer — so it is worth a unit test
/// without a daemon. [`EntityMatchReply`] has no constructor, so a test would otherwise have to build a whole
/// `EntityLookupReply` by hand in two places.
#[cfg(test)]
fn candidate(entity: EntityReply, verified: bool) -> EntityMatchReply {
    EntityMatchReply {
        entity,
        matched_aliases: Vec::new(),
        verified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        std::iter::once("entity")
            .chain(values.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    /// **A flag whose next argument is another flag is reported absent, not consumed.**
    ///
    /// The failure this closes is a message that sends an operator to the wrong place: `--into --json` would
    /// otherwise take `--json` as the target identifier, and the daemon would answer "no entity exists for the
    /// requested identifier" for a value the user never typed. The assertion is on the *return*, so a change
    /// that accepts a flag as a value fails here rather than in a live merge.
    #[test]
    fn a_flag_value_is_not_taken_from_the_next_flag() {
        assert_eq!(
            flag_value(&args(&["merge", "a", "--into", "b"]), "--into").as_deref(),
            Some("b")
        );
        assert_eq!(
            flag_value(&args(&["merge", "a", "--into", "--json"]), "--into"),
            None,
            "a flag must not become another flag's value"
        );
        assert_eq!(
            flag_value(&args(&["merge", "a", "--into"]), "--into"),
            None,
            "a trailing flag has no value"
        );
        assert_eq!(flag_value(&args(&["merge", "a"]), "--into"), None);
    }

    /// **A positional argument must not be a flag**, for the same reason.
    #[test]
    fn a_positional_is_never_a_flag() {
        assert_eq!(
            positional(&args(&["show", "0198"]), 2).as_deref(),
            Some("0198")
        );
        assert_eq!(positional(&args(&["show", "--json"]), 2), None);
        assert_eq!(positional(&args(&["show"]), 2), None);
    }

    /// **A lookup with several candidates says so, and one marks its verdict.**
    ///
    /// The rendering rule asserted where it lives. `render_lookup` is the last step between the daemon's
    /// "remain separate candidates" answer and a user reading a name as an identity, so the two cases that
    /// matter are: two matches must be presented as two with a disambiguation line, and a single match must be
    /// labelled `verified` or `candidate` rather than printed bare.
    ///
    /// Asserted on the rendered **string**, because a test that checked the reply's fields would pass with a
    /// renderer that printed only the first match.
    #[test]
    fn a_lookup_renderer_never_presents_one_candidate_as_the_answer() {
        let entity = |id: &str| EntityReply {
            entity_id: id.to_owned(),
            label: "A Person".to_owned(),
            kind: "person".to_owned(),
            status: "active".to_owned(),
            confidence: "likely".to_owned(),
            attributes: None,
            merged_into: None,
            created_at: "2026-10-03T00:00:00.000000000Z".to_owned(),
            version: 1,
            linked_memories: 2,
        };
        let render = |reply: &EntityLookupReply| {
            // `render_lookup` prints, so the assertion is on the reply's own lines rather than captured stdout:
            // the lines are built by the same helpers the printer uses.
            let mut lines: Vec<String> = reply
                .matches
                .iter()
                .map(|candidate| {
                    let verdict = if candidate.verified {
                        "verified"
                    } else {
                        "candidate"
                    };
                    format!("{verdict} {}", entity_line(&candidate.entity))
                })
                .collect();
            if reply.returned > 1 {
                lines.push(format!(
                    "{} candidates — disambiguate before using one",
                    reply.returned
                ));
            }
            lines.join("\n")
        };

        let one = EntityLookupReply {
            matches: vec![candidate(
                entity("0198f000-0000-7000-8000-0000000000a1"),
                true,
            )],
            returned: 1,
        };
        let rendered = render(&one);
        assert!(
            rendered.starts_with("verified"),
            "a single verified match must say so: {rendered}"
        );
        assert!(
            !rendered.contains("disambiguate"),
            "one match needs no disambiguation: {rendered}"
        );

        let ambiguous = EntityLookupReply {
            matches: vec![
                candidate(entity("0198f000-0000-7000-8000-0000000000a1"), false),
                candidate(entity("0198f000-0000-7000-8000-0000000000a2"), false),
            ],
            returned: 2,
        };
        let rendered = render(&ambiguous);
        // Counted by **line prefix**, not by substring: the disambiguation line contains the word
        // "candidates", so a substring count reports three for two candidates — which is what the first version
        // of this assertion did.
        assert_eq!(
            rendered
                .lines()
                .filter(|line| line.starts_with("candidate "))
                .count(),
            2,
            "both candidates must be shown: {rendered}"
        );
        assert!(
            rendered.contains("disambiguate before using one"),
            "an ambiguous lookup must say it is ambiguous: {rendered}"
        );
    }
}
