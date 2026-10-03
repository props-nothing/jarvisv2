//! The `jarvis approvals` verbs: see what a held action would do, and decide it.
//!
//! # Why these exist
//!
//! A held tool call parks its run until a person decides, and until this group there was no way for a person to
//! do that from the shipped product: the decision route existed, but nothing listed what was waiting, nothing
//! showed **what** it was waiting to do, and the nonce that authorizes a decision is delivered to a file the
//! person had to find and read by hand. An approval flow nobody can complete is a refusal that looks like a
//! feature.
//!
//! # What a person sees before deciding
//!
//! The tool, its risk, and the **arguments** — the exact URL, the exact path. Approving "a tool wants to run"
//! is approving nothing, so `approve` prints the arguments and asks, and without a terminal it refuses unless
//! `--yes` says the caller has already looked. A pending approval whose arguments were too large to hold has
//! none to show, and `approve` refuses it for that reason: nobody can approve what they cannot see.
//!
//! # Where the nonce comes from
//!
//! From the profile-private file the daemon delivered it to (`ADR-0042`), which only this account can read.
//! It is read, not taken: the daemon discards it after a successful decision, and a decision that fails for a
//! retryable reason (a typo, a lapse) must not also destroy the means to try again.
//!
//! # The CLI decides nothing
//!
//! It never names an approver — the daemon records its own identity (`jarvis_protocol::ApprovalDecisionBody`
//! has no field for one) — and it never judges whether an approval is acceptable. It shows, asks, and relays.

use std::io::{BufRead, IsTerminal, Write};

use jarvis_protocol::{
    ApprovalDecisionBody, ApprovalDecisionRequest, ApprovalListReply, PendingApprovalReply,
};
use jarvis_storage::{AppPaths, SecretStore};

use crate::api_client::{ApiClient, ApiError};
use crate::output::ExitStatus;

/// Runs one approvals verb.
pub async fn run(client: &ApiClient, paths: &AppPaths, arguments: &[String]) -> ExitStatus {
    let json = crate::json_requested(arguments);
    match arguments.get(1).map(String::as_str) {
        Some("list") | None => list(client, json).await,
        Some("approve") => decide(client, paths, arguments, ApprovalDecisionRequest::Approve).await,
        Some("deny") => decide(client, paths, arguments, ApprovalDecisionRequest::Deny).await,
        Some(other) => {
            eprintln!("jarvis: unknown approvals command {other:?}");
            eprintln!("{}", usage());
            ExitStatus::Usage
        }
    }
}

/// The usage line for this group.
pub(crate) const fn usage() -> &'static str {
    "usage: jarvis approvals <list|approve|deny> [...]\n       jarvis approvals list [--json]\n       jarvis approvals approve [APPROVAL_ID] [--yes]\n       jarvis approvals deny [APPROVAL_ID]"
}

/// `jarvis approvals list`
async fn list(client: &ApiClient, json: bool) -> ExitStatus {
    let reply = match client.list_approvals().await {
        Ok(reply) => reply,
        Err(error) => return report(&error),
    };
    if json {
        return match serde_json::to_string_pretty(&reply) {
            Ok(rendered) => {
                println!("{rendered}");
                ExitStatus::Ok
            }
            Err(error) => {
                eprintln!("jarvis: the reply could not be rendered: {error}");
                ExitStatus::Internal
            }
        };
    }
    render_list(&reply);
    ExitStatus::Ok
}

fn render_list(reply: &ApprovalListReply) {
    if reply.approvals.is_empty() {
        println!("nothing is waiting for approval");
        return;
    }
    println!("{} approval(s) waiting", reply.total);
    for approval in &reply.approvals {
        println!(
            "  {}  risk {}  {} {}  expires {}",
            approval.approval_id,
            approval.risk_level,
            approval.tool,
            approval.tool_version,
            approval.expires_at
        );
        println!("      {}", describe_arguments(approval));
    }
}

/// Renders the arguments an approval is waiting on, or says plainly that there are none to show.
///
/// A multi-line string value — a program, say — is shown as the lines it is, indented, because a person asked to
/// approve code reads it as code and not as one line full of `\n`. Everything else is shown as JSON.
fn describe_arguments(approval: &PendingApprovalReply) -> String {
    let Some(arguments) = approval.arguments.as_ref() else {
        return "arguments: not held (too large to show), so this cannot be approved here"
            .to_owned();
    };
    let Some(object) = arguments.as_object() else {
        return format!("arguments: {arguments}");
    };
    let multiline = object
        .values()
        .any(|value| value.as_str().is_some_and(|text| text.contains('\n')));
    if !multiline {
        return format!("arguments: {arguments}");
    }
    let mut lines = vec!["arguments:".to_owned()];
    for (key, value) in object {
        match value.as_str() {
            Some(text) if text.contains('\n') => {
                lines.push(format!("    {key}:"));
                lines.extend(text.lines().map(|line| format!("      | {line}")));
            }
            _ => lines.push(format!("    {key}: {value}")),
        }
    }
    lines.join("\n")
}

/// `jarvis approvals approve|deny [ID] [--yes]`
async fn decide(
    client: &ApiClient,
    paths: &AppPaths,
    arguments: &[String],
    decision: ApprovalDecisionRequest,
) -> ExitStatus {
    let pending = match client.list_approvals().await {
        Ok(reply) => reply,
        Err(error) => return report(&error),
    };
    let requested = arguments
        .iter()
        .skip(2)
        .find(|argument| !argument.starts_with("--") && !is_flag_value(arguments, argument));
    let approval = match select(&pending, requested.map(String::as_str)) {
        Ok(approval) => approval,
        Err(message) => {
            eprintln!("jarvis: {message}");
            return ExitStatus::Rejected;
        }
    };

    let approving = decision == ApprovalDecisionRequest::Approve;
    if approving && approval.arguments.is_none() {
        eprintln!(
            "jarvis: {} was held without its arguments, so there is nothing to show you and it will not be approved from here",
            approval.approval_id
        );
        return ExitStatus::Rejected;
    }

    println!(
        "{} {} (risk {}, run {})",
        if approving { "approve" } else { "deny" },
        approval.tool,
        approval.risk_level,
        approval.run_id
    );
    println!("  {}", describe_arguments(approval));
    if approving && !confirmed(arguments) {
        eprintln!("jarvis: not approved");
        return ExitStatus::Rejected;
    }

    let nonce = match read_nonce(paths, &approval.approval_id) {
        Ok(nonce) => nonce,
        Err(message) => {
            eprintln!("jarvis: {message}");
            return ExitStatus::Rejected;
        }
    };
    let body = ApprovalDecisionBody { decision, nonce };
    let decided = match client.decide_approval(&approval.approval_id, &body).await {
        Ok(decided) => decided,
        Err(error) => return report(&error),
    };
    eprintln!(
        "jarvis: {} {} (state {})",
        decided.tool,
        decided.outcome.as_deref().unwrap_or("decided"),
        decided.state
    );
    if !approving {
        // The daemon tells the parked run it was refused, so the run answers instead of waiting forever.
        return crate::chat::follow_run(client, &approval.run_id, &approval.approval_id).await;
    }

    // The decision released the call; resuming it runs the effect once and lets the parked run answer. The
    // arguments are sent from here because the daemon recomputes the intent from them, so a payload that is not
    // the one that was approved is refused rather than run.
    let Some(arguments_value) = approval.arguments.as_ref() else {
        return ExitStatus::Internal;
    };
    if let Err(error) = client.resume_call(&approval.call_id, arguments_value).await {
        return report(&error);
    }
    crate::chat::follow_run(client, &approval.run_id, &approval.approval_id).await
}

/// Picks the approval a verb applies to: the one named, or the only one pending.
fn select<'a>(
    pending: &'a ApprovalListReply,
    requested: Option<&str>,
) -> Result<&'a PendingApprovalReply, String> {
    match requested {
        Some(id) => pending
            .approvals
            .iter()
            .find(|approval| approval.approval_id == id)
            .ok_or_else(|| format!("no pending approval has the identifier {id}")),
        None => match pending.approvals.as_slice() {
            [] => Err("nothing is waiting for approval".to_owned()),
            [only] => Ok(only),
            _ => Err(
                "more than one approval is waiting; name one (see `jarvis approvals list`)"
                    .to_owned(),
            ),
        },
    }
}

/// Whether a bare argument is the value of a flag that takes one, so it is not mistaken for an identifier.
fn is_flag_value(arguments: &[String], candidate: &str) -> bool {
    arguments
        .windows(2)
        .any(|pair| pair[0] == "--root" && pair[1] == candidate)
}

/// Whether the caller has confirmed an approval: `--yes`, or an answer at a terminal.
///
/// Without either, an approval is refused. A script that pipes `yes` into this must say so with a flag rather
/// than by accident, because an approval is the one prompt that must not be satisfiable by default.
fn confirmed(arguments: &[String]) -> bool {
    if arguments.iter().any(|argument| argument == "--yes") {
        return true;
    }
    if !std::io::stdin().is_terminal() {
        eprintln!("jarvis: approving needs a terminal to confirm at, or --yes");
        return false;
    }
    eprint!("approve? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    if std::io::stdin().lock().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Reads the nonce the daemon delivered for an approval.
fn read_nonce(paths: &AppPaths, approval_id: &str) -> Result<String, String> {
    let store = SecretStore::in_state(paths.state());
    let path = store
        .path_for(approval_id)
        .map_err(|_| "the approval identifier is not usable".to_owned())?;
    let text = std::fs::read_to_string(&path).map_err(|_| {
        "no decision nonce was delivered for this approval under this profile (it may already be \
         decided, or belong to a different --root)"
            .to_owned()
    })?;
    let nonce = text.trim().to_owned();
    if nonce.is_empty() {
        return Err("the delivered nonce is empty".to_owned());
    }
    Ok(nonce)
}

fn report(error: &ApiError) -> ExitStatus {
    match error {
        ApiError::Refused(wire) => {
            eprintln!("jarvis: daemon error: {wire}");
            ExitStatus::from_code(wire.code)
        }
        other => {
            eprintln!("jarvis: {other}");
            ExitStatus::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_core::UtcTimestamp;

    fn approval(id: &str, arguments: Option<serde_json::Value>) -> PendingApprovalReply {
        PendingApprovalReply {
            approval_id: id.to_owned(),
            run_id: "run".to_owned(),
            call_id: "call".to_owned(),
            tool: "jarvis.web.fetch".to_owned(),
            tool_version: "1.0.0".to_owned(),
            risk_level: 2,
            required_strength: "credential".to_owned(),
            preview: "jarvis.web.fetch 1.0.0".to_owned(),
            arguments,
            created_at: UtcTimestamp::from_unix_nanos(1_774_000_000_000_000_000)
                .unwrap_or_else(|error| panic!("{error}")),
            expires_at: UtcTimestamp::from_unix_nanos(1_774_000_900_000_000_000)
                .unwrap_or_else(|error| panic!("{error}")),
        }
    }

    fn list_of(approvals: Vec<PendingApprovalReply>) -> ApprovalListReply {
        ApprovalListReply {
            total: approvals.len(),
            approvals,
        }
    }

    #[test]
    fn the_only_pending_approval_is_chosen_when_none_is_named() {
        let pending = list_of(vec![approval("a", None)]);
        assert_eq!(
            select(&pending, None).map(|found| found.approval_id.as_str()),
            Ok("a")
        );
    }

    #[test]
    fn two_pending_approvals_are_never_guessed_between() {
        let pending = list_of(vec![approval("a", None), approval("b", None)]);
        assert!(select(&pending, None).is_err());
        assert_eq!(
            select(&pending, Some("b")).map(|found| found.approval_id.as_str()),
            Ok("b")
        );
    }

    #[test]
    fn an_unknown_or_absent_approval_is_refused_with_a_reason() {
        let pending = list_of(vec![]);
        assert!(select(&pending, None).is_err());
        assert!(select(&list_of(vec![approval("a", None)]), Some("zzz")).is_err());
    }

    #[test]
    fn arguments_that_were_not_held_are_said_to_be_missing_rather_than_shown_empty() {
        let missing = describe_arguments(&approval("a", None));
        assert!(missing.contains("not held"), "{missing}");
        let shown = describe_arguments(&approval(
            "a",
            Some(serde_json::json!({"url": "https://example.com"})),
        ));
        assert!(shown.contains("https://example.com"), "{shown}");
    }

    #[test]
    fn a_multiline_program_is_shown_as_lines_not_as_an_escaped_string() {
        let shown = describe_arguments(&approval(
            "a",
            Some(serde_json::json!({"code": "let a = 1;\nconsole.log(a);"})),
        ));
        assert!(shown.contains("| let a = 1;"), "{shown}");
        assert!(shown.contains("| console.log(a);"), "{shown}");
        assert!(
            !shown.contains("\\n"),
            "no escaped newline may remain: {shown}"
        );
    }

    #[test]
    fn a_value_that_follows_root_is_not_mistaken_for_an_approval_id() {
        let arguments: Vec<String> = ["approvals", "approve", "--root", "C:/x"]
            .iter()
            .map(|value| (*value).to_owned())
            .collect();
        assert!(is_flag_value(&arguments, "C:/x"));
        assert!(!is_flag_value(&arguments, "approve"));
    }
}
