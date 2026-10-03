//! The `jarvis skills` verbs: the `FR-MEM-005` lifecycle applied to a stored procedure.
//!
//! # Why a subcommand group, for the reason the memory verbs are one
//!
//! `jarvis promote` and `jarvis disable` would put one feature's spelling into the global namespace, where a
//! later verb could collide with it, and a user would have to learn which commands are skill-shaped. One group
//! with sub-verbs makes the whole feature discoverable from one place.
//!
//! # What the verbs do NOT decide, and this is the point of the whole surface
//!
//! Not one of them decides whether a procedure may be used, which tools it grants, or whether a promotion is
//! acceptable — those are the daemon's, and a client that re-derived them would be a second implementation of
//! a rule that already exists in one place. Two consequences are visible in the shapes below:
//!
//! - **`--version` is required on every control verb and is never fetched.** A verb that read the current
//!   counter and sent it back would make a promotion apply to whatever the revision says *now*, which is
//!   precisely the lost update the guard exists to prevent. So the caller supplies the counter it observed,
//!   and `show` is where it comes from.
//! - **A creation states no authority.** There is no flag for a granted tool, a scope, or an approver, because
//!   `ADR-0117`'s central rule is that a skill names already-granted tools and carries none of its own.
//!
//! # Why `--tool` is repeated rather than a comma-separated list
//!
//! A tool identifier contains a dot and could contain a comma; splitting a caller's value on a character it
//! might legitimately include is the same defect the memory verbs refuse for an entity identifier. Repetition
//! is unambiguous, and the position is the array index — which the daemon requires explicitly rather than
//! inferring, so "which of these two runs first" cannot depend on how a collection was iterated.

use crate::api_client::{ApiClient, ApiError};
use crate::output::ExitStatus;
use jarvis_protocol::{
    CreateSkillRequest, CreateSkillStep, ForgetSkillRequest, PromoteSkillRequest,
    SkillDeletionReceipt, SkillDetailReply, SkillExportReply, SkillListReply, SkillReply,
    SkillTransitionRequest,
};

/// Maximum revisions one listing asks for.
///
/// Sent explicitly rather than left absent, so the page a user sees is the page the client asked for. The
/// daemon's own bound is the authority; this makes the client's expectation visible in the request.
const DEFAULT_PAGE: u32 = 50;

/// Runs one skill verb.
pub async fn run(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let json = crate::json_requested(arguments);
    match arguments.get(1).map(String::as_str) {
        Some("list") | None => list(client, json).await,
        Some("show") => show(client, arguments, json).await,
        Some("create") => create(client, arguments, json).await,
        Some("promote") => promote(client, arguments, json).await,
        Some("disable") => transition(client, arguments, json, Transition::Disable).await,
        Some("enable") => transition(client, arguments, json, Transition::Enable).await,
        Some("forget") => forget(client, arguments, json).await,
        Some("export") => export(client, json).await,
        Some(other) => {
            eprintln!("jarvis: unknown skills command {other:?}");
            eprintln!("{}", usage());
            ExitStatus::Usage
        }
    }
}

/// `jarvis skills [list]`
async fn list(client: &ApiClient, json: bool) -> ExitStatus {
    match client.list_skills(Some(DEFAULT_PAGE)).await {
        Ok(reply) => {
            render_list(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("list", &error),
    }
}

/// `jarvis skills show <id>`
async fn show(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(revision_id) = positional(arguments, 2) else {
        eprintln!("jarvis: skills show requires a revision identifier");
        return ExitStatus::Usage;
    };
    match client.read_skill(&revision_id).await {
        Ok(reply) => {
            render_detail(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("show", &error),
    }
}

/// `jarvis skills create --tool TOOL:VERSION --step TEXT --tool ... [--description TEXT] [--supersedes ID]`
///
/// # Why steps are supplied as paired repeated flags
///
/// A step is a tool and an instruction, and they must stay paired. Two separate repeated flags
/// (`--tool` and `--step`) would be zipped by position, which silently mispairs on a missing value — and a
/// mispaired step is a procedure that asks the right tool to do the wrong thing. So the pairing is explicit
/// in the flag's own value: `--tool jarvis.files.read:1.0.0` binds the version to the tool, and `--step` takes
/// the instruction that belongs to the `--tool` most recently given.
async fn create(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(description) = flag_value(arguments, "--description") else {
        eprintln!("jarvis: skills create requires --description");
        return ExitStatus::Usage;
    };
    let steps = match collect_steps(arguments) {
        Ok(steps) if steps.is_empty() => {
            eprintln!(
                "jarvis: skills create requires at least one `--tool TOOL:VERSION --step TEXT` pair; a \
                 procedure with no steps is not one"
            );
            return ExitStatus::Usage;
        }
        Ok(steps) => steps,
        Err(message) => {
            eprintln!("jarvis: {message}");
            return ExitStatus::Usage;
        }
    };

    let request = CreateSkillRequest {
        description,
        steps,
        author_version: flag_value(arguments, "--author-version").unwrap_or_else(|| "1".to_owned()),
        supersedes: flag_value(arguments, "--supersedes"),
        // Supplied only for a correction, and the daemon refuses it otherwise: a new procedure's skill
        // identity is issued by the platform, so a caller naming one could collide with something else.
        skill_id: flag_value(arguments, "--skill"),
    };
    match client.create_skill(&request).await {
        Ok(reply) => {
            render_outcome(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("create", &error),
    }
}

/// Reads the `--tool`/`--step` pairs from the arguments, in the order given.
///
/// Each `--tool` opens a step; each `--step` fills the one most recently opened. A `--step` with no `--tool`
/// before it, a `--tool` with no `--step` after it, or a `--tool` with no `:` are all reported rather than
/// defaulted — every one of them would otherwise become a step whose meaning the caller did not choose.
fn collect_steps(arguments: &[String]) -> Result<Vec<CreateSkillStep>, String> {
    let mut steps: Vec<CreateSkillStep> = Vec::new();
    let mut cursor = 0;
    while cursor < arguments.len() {
        match arguments[cursor].as_str() {
            "--tool" => {
                let Some(value) = arguments.get(cursor + 1) else {
                    return Err("--tool requires TOOL:VERSION".to_owned());
                };
                let (tool, version) = value.split_once(':').ok_or_else(|| {
                    format!(
                        "--tool {value:?} must be TOOL:VERSION, for example jarvis.files.read:1.0.0"
                    )
                })?;
                steps.push(CreateSkillStep {
                    // One-based and in the order given, so the sequence is what the caller wrote. The daemon
                    // refuses a duplicate, which would make "which runs first" depend on iteration order.
                    position: u16::try_from(steps.len() + 1).unwrap_or(u16::MAX),
                    tool: tool.to_owned(),
                    tool_version: version.to_owned(),
                    instruction: String::new(),
                });
                cursor += 2;
            }
            "--step" => {
                let Some(value) = arguments.get(cursor + 1) else {
                    return Err("--step requires the instruction text".to_owned());
                };
                let Some(step) = steps.last_mut() else {
                    return Err(
                        "--step must follow a --tool; every step belongs to the tool it names"
                            .to_owned(),
                    );
                };
                if !step.instruction.is_empty() {
                    return Err(
                        "a --tool may take one --step; give the tool again for a second step, so the pair \
                         stays explicit"
                            .to_owned(),
                    );
                }
                step.instruction.clone_from(value);
                cursor += 2;
            }
            _ => cursor += 1,
        }
    }
    if let Some(incomplete) = steps.iter().find(|step| step.instruction.is_empty()) {
        return Err(format!(
            "--tool {} has no --step; a step without an instruction is not a procedure",
            incomplete.tool
        ));
    }
    Ok(steps)
}

/// `jarvis skills promote <id> --version N --approver ACTOR`
///
/// # Why the approver is required
///
/// `ADR-0117` §4 makes promotion an **approval**, and `ADR-0043` requires a decision to name who made it. The
/// daemon refuses an approver equal to the revision's author, and that refusal is only meaningful because the
/// author comes from the authenticated session rather than from this flag — so a caller cannot supply an
/// author that differs from its approver. Requiring the flag here keeps the decision attributable on the wire
/// as well as in the store.
async fn promote(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(revision_id) = positional(arguments, 2) else {
        eprintln!("jarvis: skills promote requires a revision identifier");
        return ExitStatus::Usage;
    };
    let Some(approver_actor_id) = flag_value(arguments, "--approver") else {
        eprintln!(
            "jarvis: skills promote requires --approver; a promotion is a decision and must name who made it"
        );
        return ExitStatus::Usage;
    };
    let Some(expected_version) = version_flag(arguments) else {
        return ExitStatus::Usage;
    };

    let request = PromoteSkillRequest {
        expected_version,
        approver_actor_id,
    };
    match client.promote_skill(&revision_id, &request).await {
        Ok(reply) => {
            render_outcome(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("promote", &error),
    }
}

/// Which of the two reversible transitions a caller asked for.
#[derive(Clone, Copy)]
enum Transition {
    /// Set the revision aside: retained for inspection, no longer offered.
    Disable,
    /// Return it to the state it held before.
    Enable,
}

/// `jarvis skills disable|enable <id> --version N`
async fn transition(
    client: &ApiClient,
    arguments: &[String],
    json: bool,
    which: Transition,
) -> ExitStatus {
    let (verb, operation) = match which {
        Transition::Disable => ("disable", "disable"),
        Transition::Enable => ("enable", "enable"),
    };
    let Some(revision_id) = positional(arguments, 2) else {
        eprintln!("jarvis: skills {verb} requires a revision identifier");
        return ExitStatus::Usage;
    };
    let Some(expected_version) = version_flag(arguments) else {
        return ExitStatus::Usage;
    };
    let request = SkillTransitionRequest { expected_version };
    let result = match which {
        Transition::Disable => client.disable_skill(&revision_id, &request).await,
        Transition::Enable => client.enable_skill(&revision_id, &request).await,
    };
    match result {
        Ok(reply) => {
            render_outcome(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report(operation, &error),
    }
}

/// `jarvis skills forget <id> --version N`
///
/// There is no `--allow-relearn`, unlike `jarvis memory forget`, and the absence is deliberate: a memory needs
/// a tombstone because ingestion is continuous and automatic, while a skill is written by an explicit request
/// — so there is no process that could resurrect one and nothing for an option to control.
async fn forget(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(revision_id) = positional(arguments, 2) else {
        eprintln!("jarvis: skills forget requires a revision identifier");
        return ExitStatus::Usage;
    };
    let Some(expected_version) = version_flag(arguments) else {
        return ExitStatus::Usage;
    };
    let request = ForgetSkillRequest { expected_version };
    match client.forget_skill(&revision_id, &request).await {
        Ok(receipt) => {
            render_receipt(&receipt, json);
            ExitStatus::Ok
        }
        Err(error) => report("forget", &error),
    }
}

/// `jarvis skills export`
async fn export(client: &ApiClient, json: bool) -> ExitStatus {
    match client.export_skills(Some(DEFAULT_PAGE)).await {
        Ok(reply) => {
            render_export(&reply, json);
            ExitStatus::Ok
        }
        Err(error) => report("export", &error),
    }
}

/// Reads the required `--version` flag, reporting the reason when it is absent.
///
/// One function rather than four copies of the same message, because the reason is the same for every control
/// verb and a divergent copy would be a divergent explanation of one rule.
fn version_flag(arguments: &[String]) -> Option<i64> {
    if let Some(version) = flag_value(arguments, "--version").and_then(|value| value.parse().ok()) {
        return Some(version);
    }
    // Not fetched from the daemon, deliberately: a verb that read the counter and sent it back would make
    // the write apply to whatever the revision says *now*, which is the lost update the guard exists to
    // prevent.
    eprintln!(
        "jarvis: --version is required, taken from `jarvis skills show`; the counter is what makes this \
         write apply to the revision you read"
    );
    None
}

/// Renders a listing, as references only.
fn render_list(reply: &SkillListReply, json: bool) {
    if json {
        print_json(reply);
        return;
    }
    if reply.skills.is_empty() {
        println!("no skills are stored");
        return;
    }
    println!(
        "{} skill revision(s) (limit {})",
        reply.returned, reply.limit
    );
    for reference in &reply.skills {
        let promoted = match &reference.promoted_by_actor_id {
            Some(actor) => format!("promoted by {actor}"),
            None => "authored".to_owned(),
        };
        let superseded = match &reference.superseded_by {
            Some(successor) => format!(", superseded by {}", short(successor)),
            None => String::new(),
        };
        println!(
            "  {}  {:<9} {:<10} {:<9} {:<24} {}{}",
            short(&reference.revision_id),
            reference.state,
            reference.source_kind,
            reference.sensitivity,
            reference.created_at,
            promoted,
            superseded,
        );
        println!("      tools: {}", reference.tool_ids.join(", "));
    }
    println!();
    println!(
        "`jarvis skills show <id>` for the procedure's text. `--version` for a control verb comes from it."
    );
}

/// Renders one revision in full.
fn render_detail(reply: &SkillDetailReply, json: bool) {
    if json {
        print_json(reply);
        return;
    }
    let reference = &reply.reference;
    println!("revision    {}", reference.revision_id);
    println!("skill       {}", reference.skill_id);
    println!("version     {}", reference.author_version);
    println!("state       {}", reference.state);
    println!("usable      {}", if reply.is_usable { "yes" } else { "no" });
    if let Some(reason) = &reply.unusable_reason {
        // The verdict and its reason come from the **domain's own** eligibility rule, so this line cannot
        // disagree with whether retrieval would offer the procedure.
        println!("not usable  {reason}");
    }
    println!("sensitivity {}", reference.sensitivity);
    println!(
        "source      {} ({})",
        reference.source_kind, reference.source_locator
    );
    println!("created     {}", reference.created_at);
    if let Some(actor) = &reference.promoted_by_actor_id {
        println!("promoted by {actor}");
    }
    if let Some(at) = &reference.promoted_at {
        println!("promoted at {at}");
    }
    if let Some(predecessor) = &reference.supersedes {
        println!("replaces    {predecessor}");
    }
    if let Some(successor) = &reference.superseded_by {
        println!("replaced by {successor}");
    }
    // The counter is what a control verb must present, so it is printed rather than left for a caller to
    // discover — a value a client needs and cannot see is a value it cannot send.
    println!("counter     {}", reference.version_counter);
    println!();
    println!("{}", reply.description);

    for step in &reply.steps {
        println!("  {}. {} ({})", step.position, step.tool, step.tool_version);
        println!("     {}", step.instruction);
    }

    if !reply.dropped_fields.is_empty() {
        // Reported because `ADR-0117` §7 requires a drop to be **visible**: a field accepted-then-ignored is
        // worse than one never accepted, and an operator cannot tell a format's intent from this platform's
        // behaviour unless the difference is shown.
        println!();
        println!("fields this platform refused from the source document:");
        for dropped in &reply.dropped_fields {
            let marker = if dropped.authority_bearing {
                "authority-bearing"
            } else {
                "informational"
            };
            println!("  {} ({}, {})", dropped.field, dropped.reason, marker);
        }
    }
}

/// Renders a control verb's reply.
fn render_outcome(reply: &SkillReply, json: bool) {
    if json {
        print_json(reply);
        return;
    }
    let reference = &reply.reference;
    println!(
        "{} {} ({})",
        reference.revision_id, reply.outcome, reference.state
    );
    if let Some(detail) = &reply.detail {
        println!("  {detail}");
    }
    println!("  counter is now {}", reference.version_counter);
}

/// Renders a deletion receipt.
fn render_receipt(receipt: &SkillDeletionReceipt, json: bool) {
    if json {
        print_json(receipt);
        return;
    }
    println!("deleted {}", receipt.revision_id);
    println!(
        "  description characters removed: {}",
        receipt.removed_description_chars
    );
    println!("  steps removed: {}", receipt.removed_steps);
    println!(
        "  dropped fields removed: {}",
        receipt.removed_dropped_fields
    );
    println!(
        "  supersession links cleared: {}",
        if receipt.cleared_supersession {
            "yes"
        } else {
            "no"
        }
    );
    println!();
    println!("this deletion could not reach:");
    for entry in &receipt.unreachable {
        println!("  - {entry}");
    }
}

/// Renders an export.
fn render_export(reply: &SkillExportReply, json: bool) {
    if json {
        print_json(reply);
        return;
    }
    println!(
        "{} revision(s) exported from {} at {}",
        reply.count, reply.workspace_id, reply.exported_at
    );
    for skill in &reply.skills {
        render_detail(
            &SkillDetailReply {
                reference: skill.reference.clone(),
                description: skill.description.clone(),
                steps: skill.steps.clone(),
                dropped_fields: skill.dropped_fields.clone(),
                is_usable: skill.reference.state == "active"
                    && skill.reference.superseded_by.is_none(),
                unusable_reason: None,
            },
            false,
        );
        println!();
    }
    println!("this export deliberately excludes:");
    for entry in &reply.exclusions {
        println!("  - {entry}");
    }
}

/// Prints a reply as JSON, so a script can read it.
fn print_json<T: serde::Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("jarvis: could not render the reply as JSON: {error}"),
    }
}

/// Reports a failure, distinguishing a refusal from an unreachable daemon.
///
/// A refusal is `Denied` and a transport failure is `Unavailable`, because they are different problems with
/// different fixes: a refusal means the daemon answered and declined, while an unreachable daemon means the
/// answer never arrived.
fn report(operation: &str, error: &ApiError) -> ExitStatus {
    eprintln!("jarvis: skills {operation} failed: {error}");
    match error {
        ApiError::Refused(_) | ApiError::Identifier(_) => ExitStatus::Denied,
        ApiError::Transport(_) | ApiError::UnexpectedStatus { .. } | ApiError::Decode => {
            ExitStatus::Unavailable
        }
    }
}

/// Returns the first positional argument after `index`, skipping flags and their values.
fn positional(arguments: &[String], index: usize) -> Option<String> {
    let mut cursor = index;
    while cursor < arguments.len() {
        let argument = &arguments[cursor];
        if argument.starts_with("--") {
            // A flag consumed its value, so both are skipped — except a boolean flag, which has none. The set
            // of boolean flags is named rather than inferred, because a flag added without a value would
            // otherwise be skipped *along with the word after it*, silently dropping the positional.
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
fn is_boolean_flag(flag: &str) -> bool {
    matches!(flag, "--json")
}

/// Returns a flag's value, or `None` when the flag is absent.
fn flag_value(arguments: &[String], flag: &str) -> Option<String> {
    let position = arguments.iter().position(|argument| argument == flag)?;
    arguments.get(position + 1).cloned()
}

/// Returns the first eight characters of an identifier, with an ellipsis when it was cut.
///
/// A listing of full v7 identifiers is unreadable, and the prefix is what a user copies into a `show`. It is
/// **not** used for a request: every verb that acts on a revision takes the full identifier, so a truncated
/// value cannot become an ambiguous target.
fn short(id: &str) -> String {
    if id.chars().count() <= 8 {
        return id.to_owned();
    }
    format!("{}…", id.chars().take(8).collect::<String>())
}

/// The usage text for the skill verbs.
#[must_use]
pub const fn usage() -> &'static str {
    "usage: jarvis skills [list]\n       \
     jarvis skills show <id>\n       \
     jarvis skills create --description TEXT --tool TOOL:VERSION --step TEXT [...] [--author-version V]\n       \
     jarvis skills create ... [--supersedes ID --skill SKILL]\n       \
     jarvis skills promote <id> --version N --approver ACTOR\n       \
     jarvis skills disable <id> --version N\n       \
     jarvis skills enable <id> --version N\n       \
     jarvis skills forget <id> --version N\n       \
     jarvis skills export"
}
