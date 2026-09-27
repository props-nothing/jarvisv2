//! The `jarvis memory` verbs.
//!
//! # Why this is a subcommand group rather than seven top-level verbs
//!
//! `jarvis remember`, `jarvis forget`, and `jarvis memory list` would put one feature's spelling into the
//! global namespace, where a later connector verb could collide with it and a user has to learn which
//! commands are memory-shaped. One group with sub-verbs keeps the surface readable and makes the whole
//! feature discoverable from one place.
//!
//! # What a verb renders, and why some of it is not content
//!
//! `list` and `search` show **references**: an identifier, a type, a source, and the timestamps. That is the
//! same rule the daemon's wire types follow, and it exists because a listing is the wrong place for text —
//! which is why `show` exists for the one case where content is the answer, and why it takes an identifier
//! rather than listing everything.
//!
//! # The verbs are thin on purpose
//!
//! Every one of them turns arguments into a request and renders a reply. No verb decides whether a claim is
//! acceptable, what confidence it supports, or whether a correction should apply — those are the daemon's,
//! and a client that re-derived them would be a second implementation of a rule that already exists in one
//! place. The `expected_version` a correction carries is passed through rather than managed, because the
//! caller is the party that read the claim.

use crate::api_client::{ApiClient, ApiError};
use crate::output::ExitStatus;
use jarvis_protocol::{
    CorrectMemoryRequest, ForgetMemoryRequest, MemoryDetailReply, MemoryExportReply,
    MemoryListReply, MemoryReference, MemorySearchHit, MemorySearchReply, MemorySearchRequest,
    RememberRequest,
};

/// Maximum claims one listing or search asks for.
///
/// Sent explicitly rather than left absent, so the page a user sees is the page the client asked for. The
/// daemon's own bound is the authority; this is what makes the client's expectation visible in the request.
const DEFAULT_PAGE: u32 = 50;

/// Runs one memory verb.
pub async fn run(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let json = crate::json_requested(arguments);
    match arguments.get(1).map(String::as_str) {
        Some("list") | None => list(client, json).await,
        Some("show") => show(client, arguments, json).await,
        Some("search") => search(client, arguments, json).await,
        Some("remember") => remember(client, arguments, json).await,
        Some("correct") => correct(client, arguments, json).await,
        Some("forget") => forget(client, arguments, json).await,
        Some("export") => export(client, arguments, json).await,
        Some(other) => {
            eprintln!("jarvis: unknown memory command {other:?}");
            eprintln!("{}", usage());
            ExitStatus::Usage
        }
    }
}

/// `jarvis memory [list]`
async fn list(client: &ApiClient, json: bool) -> ExitStatus {
    match client.list_memories(Some(DEFAULT_PAGE)).await {
        Ok(reply) => {
            render_list(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("list", &error),
    }
}

/// `jarvis memory show <id>`
async fn show(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(memory_id) = positional(arguments, 2) else {
        eprintln!("jarvis: memory show requires a memory identifier");
        return ExitStatus::Usage;
    };
    match client.read_memory(&memory_id).await {
        Ok(reply) => {
            render_detail(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("show", &error),
    }
}

/// `jarvis memory search <text...>`
async fn search(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let words = trailing_words(arguments, 2);
    if words.is_empty() {
        eprintln!("jarvis: memory search requires some text to look for");
        return ExitStatus::Usage;
    }
    let request = MemorySearchRequest {
        text: Some(words.join(" ")),
        memory_types: Vec::new(),
        entity_ids: Vec::new(),
        minimum_trust: None,
        minimum_confidence: None,
        limit: Some(DEFAULT_PAGE),
    };
    match client.search_memories(&request).await {
        Ok(reply) => {
            render_search(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("search", &error),
    }
}

/// `jarvis memory remember <text...> --type T --source S --entity ID [--supersedes ID]`
///
/// # Why the flags are required rather than defaulted
///
/// A claim's type and source decide its confidence cap, its sensitivity floor, and whether it may be offered
/// as a fact. A default would let `jarvis memory remember "..."` store a claim under a combination the user
/// did not choose, and the wrong pair is not an error the store can detect — a `Semantic` claim from a
/// `UserStatement` is perfectly storable and simply not what was meant. Requiring both makes the user state
/// what they know, which for a memory is the one thing only they can supply.
async fn remember(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(content) = flag_value(arguments, "--content").or_else(|| free_text(arguments, 2))
    else {
        eprintln!("jarvis: memory remember requires the claim, for example:");
        eprintln!("         jarvis memory remember --type preference --source user_statement \\");
        eprintln!("           --entity <uuid> \"prefers dark roast coffee\"");
        return ExitStatus::Usage;
    };
    let Some(memory_type) = flag_value(arguments, "--type") else {
        eprintln!(
            "jarvis: memory remember requires --type (for example preference, semantic, episodic)"
        );
        return ExitStatus::Usage;
    };
    let Some(source_kind) = flag_value(arguments, "--source") else {
        eprintln!(
            "jarvis: memory remember requires --source (for example user_statement, document, tool_observation)"
        );
        return ExitStatus::Usage;
    };
    let entity_ids = flag_values(arguments, "--entity");
    if entity_ids.is_empty() {
        // Refused here with the reason, rather than letting the daemon answer `422`: a memory must name an
        // entity, and the client is the party that can supply one.
        eprintln!(
            "jarvis: memory remember requires at least one --entity; a claim must be about something"
        );
        return ExitStatus::Usage;
    }

    let request = RememberRequest {
        content,
        memory_type,
        source_kind,
        importance: flag_value(arguments, "--importance").and_then(|value| value.parse().ok()),
        entity_ids,
        claim: None,
        supersedes: flag_value(arguments, "--supersedes"),
    };
    match client.remember(&request).await {
        Ok(reply) => {
            render_outcome(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("remember", &error),
    }
}

/// `jarvis memory correct <id> --version N <text...>`
async fn correct(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(memory_id) = positional(arguments, 2) else {
        eprintln!("jarvis: memory correct requires a memory identifier");
        return ExitStatus::Usage;
    };
    let Some(expected_version) = flag_value(arguments, "--version").and_then(|v| v.parse().ok())
    else {
        // Required, and its absence is a usage error rather than a re-read: the client that did not read the
        // claim has no basis for overwriting what a user currently believes, and fetching a version here
        // would make a correction silently apply to whatever the claim says *now*.
        eprintln!("jarvis: memory correct requires --version, taken from `jarvis memory show`");
        return ExitStatus::Usage;
    };
    let Some(content) = free_text(arguments, 2).or_else(|| flag_value(arguments, "--content"))
    else {
        eprintln!("jarvis: memory correct requires the replacement text");
        return ExitStatus::Usage;
    };

    let request = CorrectMemoryRequest {
        content,
        expected_version,
        entity_ids: None,
    };
    match client.correct_memory(&memory_id, &request).await {
        Ok(reply) => {
            render_outcome(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("correct", &error),
    }
}

/// `jarvis memory forget <id> --version N [--allow-relearn]`
async fn forget(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(memory_id) = positional(arguments, 2) else {
        eprintln!("jarvis: memory forget requires a memory identifier");
        return ExitStatus::Usage;
    };
    let Some(expected_version) = flag_value(arguments, "--version").and_then(|v| v.parse().ok())
    else {
        eprintln!("jarvis: memory forget requires --version, taken from `jarvis memory show`");
        return ExitStatus::Usage;
    };
    let request = ForgetMemoryRequest {
        expected_version,
        allow_relearn: arguments.iter().any(|arg| arg == "--allow-relearn"),
    };
    match client.forget_memory(&memory_id, &request).await {
        Ok(receipt) => {
            render_receipt(&receipt, json);
            ExitStatus::Ok
        }
        Err(error) => report("forget", &error),
    }
}

/// `jarvis memory export [--limit N] [--offset N]`
async fn export(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let limit = flag_value(arguments, "--limit")
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PAGE);
    let offset = flag_value(arguments, "--offset").and_then(|value| value.parse().ok());
    match client.export_memories(Some(limit), offset).await {
        Ok(reply) => {
            render_export(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("export", &error),
    }
}

/// Renders a listing.
fn render_list(reply: &MemoryListReply, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(reply).unwrap_or_default()
        );
        return;
    }
    if reply.memories.is_empty() {
        println!("No memories have been recorded in this workspace.");
        return;
    }
    println!("{} of at most {} memories:", reply.returned, reply.limit);
    for memory in &reply.memories {
        print_reference(memory);
    }
}

/// Renders one reference as a line.
fn print_reference(memory: &MemoryReference) {
    // The effective status is printed rather than the stored one when they differ, because the effective
    // value is what is true: an active claim whose window lapsed is not retrievable, and showing `active`
    // would tell a user their memory is in force when it is not.
    let status = if memory.effective_status == memory.status {
        memory.status.clone()
    } else {
        format!("{} ({})", memory.effective_status, memory.status)
    };
    println!(
        "  {}  {}  {}  {}  {:.1}",
        short(&memory.memory_id),
        memory.memory_type,
        status,
        memory.source_kind,
        // Shown because it is the field that decides whether this may be stated as a fact, and a user
        // asking "do you believe this" deserves the answer on the line rather than behind another command.
        memory.confidence
    );
}

/// Renders one claim in full, which is the one place content is displayed.
fn render_detail(reply: &MemoryDetailReply, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(reply).unwrap_or_default()
        );
        return;
    }
    println!("memory      {}", reply.reference.memory_id);
    println!("type        {}", reply.reference.memory_type);
    println!("status      {}", reply.reference.effective_status);
    println!("confidence  {}", reply.reference.confidence);
    println!("sensitivity {}", reply.reference.sensitivity);
    println!(
        "source      {} ({})",
        reply.reference.source_kind, reply.source_locator
    );
    println!("recorded    {}", reply.reference.created_at);
    println!("version     {}", reply.reference.retrieval_count);
    if let Some(superseded_by) = &reply.reference.superseded_by {
        println!("replaced by {superseded_by}");
    }
    // Both predicates are rendered, because together they answer the two questions a user actually has:
    // may this be stated as fact, and would a model read it as their own words.
    println!(
        "stated as fact   {}",
        if reply.is_stated_as_fact { "yes" } else { "no" }
    );
    println!(
        "read as untrusted data  {}",
        if reply.carries_untrusted_trust {
            "yes"
        } else {
            "no"
        }
    );
    if !reply.entity_ids.is_empty() {
        println!("entities    {}", reply.entity_ids.join(", "));
    }
    println!();
    println!("{}", reply.content);
}

/// Renders a search, including why each match ranked where it did.
fn render_search(reply: &MemorySearchReply, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(reply).unwrap_or_default()
        );
        return;
    }
    if reply.matches.is_empty() {
        println!("No memory matched, out of {} considered.", reply.considered);
        return;
    }
    println!(
        "{} match(es) out of {} considered:",
        reply.matches.len(),
        reply.considered
    );
    for matched in &reply.matches {
        print_hit(matched);
    }
}

/// Renders one ranked hit with its score and the components that produced it.
///
/// The explanation is printed because `docs/architecture/memory-and-context.md` requires a user be able to
/// ask why something was used and be answered from stored selection reasons. Printing only the score would
/// leave the answer unreachable from the command that produced the ranking.
fn print_hit(matched: &MemorySearchHit) {
    print_reference(&matched.reference);
    let components: Vec<String> = matched
        .contributions
        .iter()
        .map(|entry| format!("{} {}", entry.signal, entry.contribution))
        .collect();
    println!(
        "      score {}  reason {}{}{}",
        matched.score,
        matched.reason,
        if matched.is_a_match {
            ""
        } else {
            " (not a match)"
        },
        if components.is_empty() {
            String::new()
        } else {
            format!("  [{}]", components.join(", "))
        }
    );
}

/// Renders the outcome of a write.
fn render_outcome(reply: &jarvis_protocol::MemoryReply, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(reply).unwrap_or_default()
        );
        return;
    }
    println!("{} {}", reply.outcome, reply.memory_id);
    println!("  type   {}", reply.memory_type);
    println!("  status {}", reply.effective_status);
    if let Some(detail) = &reply.detail {
        println!("  {detail}");
    }
    println!(
        "  version {} — pass this to `memory correct` or `memory forget`",
        reply.version
    );
}

/// Renders a deletion receipt.
///
/// The **unreachable** list is printed, and prominently: a receipt that said only "deleted" would be
/// indistinguishable from one for a claim that never existed, and
/// `docs/architecture/memory-and-context.md` requires that provider-side copies be "surfaced separately"
/// rather than implied. A user who believes a deletion was total when it was not has been misled by the
/// command that was supposed to inform them.
fn render_receipt(receipt: &jarvis_protocol::DeletionReceipt, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(receipt).unwrap_or_default()
        );
        return;
    }
    println!("Deleted {}.", receipt.memory_id);
    println!(
        "  removed {} characters of text",
        receipt.removed_content_chars
    );
    println!(
        "  search key removed: {}",
        if receipt.removed_search_key {
            "yes"
        } else {
            "no"
        }
    );
    println!(
        "  tombstone written:  {}",
        if receipt.tombstone_written {
            "yes (a re-ingest of this claim will be blocked)"
        } else {
            "no (the claim may be learned again)"
        }
    );
    println!("  entity links removed: {}", receipt.removed_entity_links);
    println!();
    println!("What this deletion could not reach:");
    for entry in &receipt.unreachable {
        println!("  - {entry}");
    }
}

/// Renders an export.
fn render_export(reply: &MemoryExportReply, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(reply).unwrap_or_default()
        );
        return;
    }
    println!(
        "{} memory record(s) for workspace {}",
        reply.count, reply.workspace_id
    );
    println!("exported at {}", reply.exported_at);
    println!();
    // The exclusions are printed **before** the records, so a reader sees what the list is not while they
    // are still forming an impression of it, rather than after they have concluded it is complete.
    println!("This export does not include:");
    for entry in &reply.exclusions {
        println!("  - {entry}");
    }
    println!();
    for memory in &reply.memories {
        println!(
            "{}  {}  {}  {}",
            short(&memory.memory_id),
            memory.memory_type,
            memory.effective_status,
            memory.source_kind
        );
        if memory.content.is_empty() {
            // A deleted claim's tombstone form, named as such rather than shown as an empty claim.
            println!("      (content removed; the deletion is recorded)");
        } else {
            println!("      {}", memory.content);
        }
    }
}

/// Reports a failure and maps it onto an exit status.
///
/// A refusal is `Denied` and a transport failure is `Unavailable`, because they are different problems with
/// different fixes: a refusal means the daemon answered and declined, while an unreachable daemon means the
/// answer never arrived. Collapsing them would send a user to check a policy when the daemon is not running.
fn report(operation: &str, error: &ApiError) -> ExitStatus {
    eprintln!("jarvis: memory {operation} failed: {error}");
    match error {
        ApiError::Refused(_) | ApiError::Identifier(_) => ExitStatus::Denied,
        ApiError::Transport(_) | ApiError::UnexpectedStatus { .. } | ApiError::Decode => {
            ExitStatus::Unavailable
        }
    }
}

/// Returns the first positional argument after `index`, skipping flags and their values.
///
/// Flags are skipped because `jarvis memory correct <id> --version 3 "text"` puts the identifier before the
/// flags and the text after them, and a positional reader that took the next word verbatim would return
/// `--version`.
fn positional(arguments: &[String], index: usize) -> Option<String> {
    let mut cursor = index;
    while cursor < arguments.len() {
        let argument = &arguments[cursor];
        if argument.starts_with("--") {
            // A flag consumed its value, so both are skipped. A boolean flag has no value, and skipping the
            // following word would drop the identifier; the boolean flags here are all known, so this only
            // treats a flag as value-taking when it is not in the boolean set.
            if !is_boolean_flag(argument) {
                cursor += 2;
                continue;
            }
            cursor += 1;
            continue;
        }
        return Some(argument.clone());
    }
    None
}

/// Returns whether a flag takes no value.
///
/// The set is closed and named here rather than inferred from a leading `--`, because a flag added without a
/// value would otherwise be skipped *along with the word after it* — silently dropping the very positional
/// the caller supplied.
///
/// Not `const`: a `&str` cannot be compared in a const context on this toolchain, so the byte-pattern match
/// this replaced needed a duplicate `&str` arm to be usable at all. Dropping `const` is what removes the
/// duplication rather than tidying it.
fn is_boolean_flag(flag: &str) -> bool {
    matches!(flag, "--allow-relearn" | "--json")
}

/// Returns every word after `index` that is not part of a flag.
fn trailing_words(arguments: &[String], index: usize) -> Vec<String> {
    let mut words = Vec::new();
    let mut cursor = index;
    while cursor < arguments.len() {
        let argument = &arguments[cursor];
        if argument.starts_with("--") {
            if !is_boolean_flag(argument) {
                cursor += 2;
                continue;
            }
            cursor += 1;
            continue;
        }
        words.push(argument.clone());
        cursor += 1;
    }
    words
}

/// Returns the joined free text after `index`, or `None` when there is none.
fn free_text(arguments: &[String], index: usize) -> Option<String> {
    let words = trailing_words(arguments, index);
    if words.is_empty() {
        return None;
    }
    Some(words.join(" "))
}

/// Returns a flag's value, or `None` when the flag is absent.
fn flag_value(arguments: &[String], flag: &str) -> Option<String> {
    let position = arguments.iter().position(|argument| argument == flag)?;
    arguments.get(position + 1).cloned()
}

/// Returns every value given for a repeated flag.
///
/// Repeated rather than comma-separated, so an entity identifier cannot be split on a character a caller
/// might legitimately include.
fn flag_values(arguments: &[String], flag: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut cursor = 0;
    while cursor < arguments.len() {
        if arguments[cursor] == flag {
            if let Some(value) = arguments.get(cursor + 1) {
                values.push(value.clone());
            }
            cursor += 2;
            continue;
        }
        cursor += 1;
    }
    values
}

/// Returns the first eight characters of an identifier, with an ellipsis when it was cut.
///
/// A listing of full v7 identifiers is unreadable, and the prefix is what a user copies into a `show`. It is
/// **not** used for a request: every verb that acts on a claim takes the full identifier, so a truncated
/// value cannot become an ambiguous target.
fn short(id: &str) -> String {
    if id.chars().count() <= 8 {
        return id.to_owned();
    }
    format!("{}…", id.chars().take(8).collect::<String>())
}

/// The usage text for the memory verbs.
#[must_use]
pub const fn usage() -> &'static str {
    "usage: jarvis memory [list]\n       \
     jarvis memory show <id>\n       \
     jarvis memory search <text...>\n       \
     jarvis memory remember --type T --source S --entity ID [--importance N] <text...>\n       \
     jarvis memory correct <id> --version N <text...>\n       \
     jarvis memory forget <id> --version N [--allow-relearn]\n       \
     jarvis memory export [--limit N] [--offset N]"
}
