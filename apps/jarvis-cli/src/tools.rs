//! The `jarvis tools` verbs: what this daemon can do, and what it would decide.
//!
//! # Why this is a subcommand group
//!
//! Same reasoning as `jarvis memory`: `jarvis tools`, `jarvis preview`, and `jarvis policy` would put one
//! feature's spelling into the global namespace, where a later verb could collide with it and a user would
//! have to learn which commands are tool-shaped. One group with sub-verbs keeps the surface readable and
//! makes the whole feature discoverable from one place.
//!
//! # Why the verbs exist at all
//!
//! `P3-025` made a tool's approval policy configurable in the daemon's document. That made a **new
//! question** answerable — *did my configuration take effect?* — and a configuration file cannot answer it,
//! because the document is the input while the workspace policy is what the engine enforces. `list` reads
//! the posture that is in force, showing the declared policy beside the effective one and naming an override
//! as an override.
//!
//! # `preview` is a decision, not a prediction
//!
//! The daemon computes it with the same pure `evaluate` the tool-call path uses, so the answer is exact for
//! the context supplied rather than an estimate. The output says so, because a user reading "deny" needs to
//! know it is the decision they would get and not a guess about one.
//!
//! # The verbs are thin on purpose
//!
//! Neither verb decides anything. `list` renders what the daemon reported and `preview` sends the context it
//! was given. A client that re-derived the direction rule, or that guessed a policy by comparing two fields,
//! would be a second implementation of a rule that already exists in one place — and the copy in the client
//! is the one that would drift.

use crate::api_client::{ApiClient, ApiError};
use crate::output::ExitStatus;
use jarvis_protocol::{ToolListReply, ToolPreviewReply, ToolPreviewRequest};

/// Runs one tools verb.
pub async fn run(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let json = crate::json_requested(arguments);
    match arguments.get(1).map(String::as_str) {
        Some("list") | None => list(client, json).await,
        Some("preview") => preview(client, arguments, json).await,
        Some(other) => {
            eprintln!("jarvis: unknown tools command {other:?}");
            eprintln!("{}", usage());
            ExitStatus::Usage
        }
    }
}

/// The usage line for this group, so an unknown sub-verb names what is accepted.
pub(crate) const fn usage() -> &'static str {
    "usage: jarvis tools <list|preview> [...]\n       jarvis tools list [--json]\n       jarvis tools preview <tool> [--escalation external|bulk|sensitive|production] [--json]"
}

/// Exposes the flag parser to this crate's tests.
///
/// A test-only door rather than a `pub` function: the parser is an implementation detail of the `preview`
/// verb, and making it public would be a surface a caller could reach without going through the verb that
/// validates the tool identifier first. The tests need it directly because the parsing is what can be wrong
/// while the rendered output looks entirely plausible.
#[cfg(test)]
pub(crate) fn preview_request_for_test(
    arguments: &[String],
) -> Result<ToolPreviewRequest, ExitStatus> {
    preview_request(arguments)
}

/// `jarvis tools list`
async fn list(client: &ApiClient, json: bool) -> ExitStatus {
    match client.list_tools().await {
        Ok(reply) => {
            if json {
                match serde_json::to_string_pretty(&reply) {
                    Ok(rendered) => {
                        println!("{rendered}");
                        ExitStatus::Ok
                    }
                    Err(error) => {
                        eprintln!("jarvis: the reply could not be rendered: {error}");
                        ExitStatus::Internal
                    }
                }
            } else {
                render_list(&reply);
                ExitStatus::Ok
            }
        }
        // The daemon answers `404` when it has no tool surface at all, and that is a **deployment** fact
        // rather than a failure: a profile with no roots grant and no MCP servers registers no tool. Printed
        // as guidance with a distinct exit code, so a script can tell "nothing configured" from "broken".
        Err(error) => report("list", &error),
    }
}

/// Renders the inventory as one aligned line per tool.
///
/// Columns rather than a paragraph, because the question this output answers is comparative: an operator
/// scanning for the one tool whose policy looks wrong reads the `approval` column down, which a prose
/// rendering would hide.
fn render_list(reply: &ToolListReply) {
    println!(
        "{} tool(s); workspace ceiling {}, approval from {}",
        reply.total, reply.max_risk, reply.approval_threshold
    );
    if reply.tools.is_empty() {
        println!("  (none registered)");
        return;
    }
    for tool in &reply.tools {
        // The effective policy is what is enforced, so it is the leading value; the declaration is shown
        // only when it differs, because an override is the fact worth seeing and two identical values are
        // noise on every other line.
        let approval = if tool.overridden {
            format!(
                "{} -> {} (overridden)",
                tool.declared_approval, tool.effective_approval
            )
        } else {
            tool.effective_approval.to_string()
        };
        let state = if tool.denied {
            "denied".to_owned()
        } else if tool.callable {
            "callable".to_owned()
        } else {
            match &tool.unavailable_reason {
                Some(reason) => format!("unavailable: {reason}"),
                None => "unavailable".to_owned(),
            }
        };
        println!(
            "  {id:<40} risk {risk:<9} approval {approval:<32} {state}",
            id = tool.id,
            risk = tool.risk.as_str(),
            approval = approval,
            state = state
        );
    }
}

/// `jarvis tools preview <tool> [flags]`
async fn preview(client: &ApiClient, arguments: &[String], json: bool) -> ExitStatus {
    let Some(tool) = arguments.get(2) else {
        eprintln!("jarvis: tools preview requires a tool identifier");
        eprintln!("{}", usage());
        return ExitStatus::Usage;
    };
    if tool.starts_with("--") {
        eprintln!("jarvis: tools preview requires a tool identifier, got the flag {tool:?}");
        eprintln!("{}", usage());
        return ExitStatus::Usage;
    }

    let request = match preview_request(arguments) {
        Ok(request) => request,
        Err(status) => return status,
    };

    match client.preview_tool(tool, &request).await {
        Ok(reply) => {
            if json {
                match serde_json::to_string_pretty(&reply) {
                    Ok(rendered) => {
                        println!("{rendered}");
                        ExitStatus::Ok
                    }
                    Err(error) => {
                        eprintln!("jarvis: the reply could not be rendered: {error}");
                        ExitStatus::Internal
                    }
                }
            } else {
                render_preview(&reply);
                ExitStatus::Ok
            }
        }
        Err(error) => report("preview", &error),
    }
}

/// Parses the preview flags into a request body.
///
/// # Why it starts at index 3
///
/// The arguments are the whole command line, so `[0]` is `tools`, `[1]` is `preview`, and `[2]` is the
/// **tool identifier** the verb already read. Starting at 2 makes the identifier look like a flag and every
/// preview fail with `unknown tools option "jarvis.files.read"` — which is what the first version of this
/// function did, and a test caught it because the parser is asserted directly rather than through the
/// rendered output.
///
/// # Why hand-parsed rather than deserialized
///
/// Matching every other verb here: the flags are a handful of short values, and a deserializer would accept a
/// body the daemon's `deny_unknown_fields` would then refuse — the client reporting a `422` for its own
/// mistake rather than naming the flag.
///
/// An unknown escalation is a **usage error** rather than an ignored value: silently dropping one would
/// compute the preview for a call *without* that signal, which reads as more permissive than the user
/// asked about.
fn preview_request(arguments: &[String]) -> Result<ToolPreviewRequest, ExitStatus> {
    let mut request = ToolPreviewRequest::default();
    let mut index = 3;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        match argument {
            "--json" => {}
            "--escalation" => {
                let Some(value) = arguments.get(index + 1) else {
                    eprintln!("jarvis: --escalation requires a value");
                    return Err(ExitStatus::Usage);
                };
                let Ok(signal) = value.parse::<jarvis_core::EscalationSignal>() else {
                    eprintln!(
                        "jarvis: unknown escalation {value:?}; accepted: external, bulk, sensitive, \
                         production"
                    );
                    return Err(ExitStatus::Usage);
                };
                request.escalation.push(signal);
                index += 1;
            }
            other => {
                eprintln!("jarvis: unknown tools option {other:?}");
                eprintln!("{}", usage());
                return Err(ExitStatus::Usage);
            }
        }
        index += 1;
    }
    Ok(request)
}

/// Renders a preview decision for a human.
fn render_preview(reply: &ToolPreviewReply) {
    // The outcome is the first line because it is the answer; everything after it is why. The wording says
    // "would" rather than "is", because this is a decision computed for the supplied context and not a
    // claim about a call that happened.
    println!("{}: {}", reply.tool, reply.decision);
    println!("  reason            {}", reply.reason);
    println!(
        "  risk              {} (declared {})",
        reply.effective_risk, reply.declared_risk
    );
    if !reply.escalated_by.is_empty() {
        let signals: Vec<&str> = reply.escalated_by.iter().map(|s| s.code()).collect();
        println!("  escalated by      {}", signals.join(", "));
    }
}

/// Reports a failure and maps it onto an exit status.
///
/// A refusal is `Denied` and a transport failure is `Unavailable`, for the reason `memory::report` states:
/// a refusal means the daemon answered and declined, while an unreachable daemon means the answer never
/// arrived. Collapsing them would send a user to check a policy when the daemon is not running.
///
/// The `NotFound` case lands in `Denied` rather than `Unavailable`, and that is deliberate: a `404` from
/// this surface means the **tool surface is absent**, which is a configuration decision the operator has to
/// make rather than a broken daemon. The message names both remedies, because the daemon cannot tell which
/// one applies.
fn report(operation: &str, error: &ApiError) -> ExitStatus {
    match error {
        ApiError::Refused(_) | ApiError::Identifier(_) => {
            eprintln!("jarvis: tools {operation} failed: {error}");
            eprintln!(
                "jarvis: no tool is registered — grant `daemon.tool_workspace_roots` or configure MCP \
                 servers in `mcp-servers.toml`"
            );
            // The two conditions above were the **whole** answer until `P4-014`, and they are no longer: the
            // memory-proposal tool is native and needs neither. A hint that lists only the old conditions sends
            // an operator to configure roots for a tool that does not read files — and the tool they are looking
            // for would still be missing, because the real cause is something else.
            eprintln!(
                "jarvis: the built-in memory tool `jarvis.memory.propose` needs neither, so it should be \
                 present on any build; if it is absent the daemon was not composed with it"
            );
        }
        other => eprintln!("jarvis: tools {operation} failed: {other}"),
    }
    match error {
        ApiError::Refused(_) | ApiError::Identifier(_) => ExitStatus::Denied,
        ApiError::Transport(_) | ApiError::UnexpectedStatus { .. } | ApiError::Decode => {
            ExitStatus::Unavailable
        }
    }
}
