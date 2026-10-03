//! `jarvis cancel`: the kill switch.
//!
//! ```text
//! jarvis cancel RUN       # stop one run (an identifier prefix is enough when it is unambiguous)
//! jarvis cancel --all     # stop everything that is running or waiting
//! ```
//!
//! A run that is still going is one whose outcome is not yet recorded. Cancellation is a **request** the run honours at
//! its next step boundary, so the command reports what it asked for, not that the run has already stopped; `jarvis runs`
//! shows when it has. A run that has already settled is left alone and said so, never "cancelled" after the fact.

use jarvis_protocol::RunSummaryReply;

use crate::api_client::{ApiClient, ApiError};
use crate::output::ExitStatus;

/// How many recent runs are searched for ones still going. The list endpoint's own maximum.
const SEARCH_LIMIT: u32 = 50;

/// Picks the runs a request names out of the recent list: every unfinished one for `--all`, or the one whose
/// identifier starts with `typed`.
///
/// A prefix that matches more than one run, or none that is still going, is an error rather than a guess: a kill
/// switch that stops the wrong thing is worse than one that asks again.
fn choose<'a>(
    runs: &'a [RunSummaryReply],
    typed: Option<&str>,
    all: bool,
) -> Result<Vec<&'a RunSummaryReply>, String> {
    let going: Vec<&RunSummaryReply> = runs.iter().filter(|run| run.outcome.is_none()).collect();
    if all {
        return Ok(going);
    }
    let Some(typed) = typed.filter(|text| !text.is_empty()) else {
        return Err("name a run, or use --all".to_owned());
    };
    let named: Vec<&RunSummaryReply> = runs
        .iter()
        .filter(|run| run.run_id.starts_with(typed))
        .collect();
    match named.as_slice() {
        [] => Err(format!("no recent run starts with {typed:?}")),
        [one] if one.outcome.is_some() => Err(format!("run {} has already finished", one.run_id)),
        [one] => Ok(vec![one]),
        many => Err(format!(
            "{} runs start with {typed:?}; type more of the identifier",
            many.len()
        )),
    }
}

/// Runs `jarvis cancel`.
pub async fn run_cancel(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let all = arguments.iter().any(|argument| argument == "--all");
    // The first word that is neither a flag nor the value of `--root`.
    let typed = arguments
        .iter()
        .enumerate()
        .skip(1)
        .find(|(index, argument)| {
            !argument.starts_with("--")
                && arguments.get(index - 1).map(String::as_str) != Some("--root")
        })
        .map(|(_, argument)| argument.as_str());
    let reply = match client.list_runs(SEARCH_LIMIT).await {
        Ok(reply) => reply,
        Err(error) => return report(&error),
    };
    let chosen = match choose(&reply.runs, typed, all) {
        Ok(chosen) => chosen,
        Err(message) => {
            eprintln!("jarvis: {message}");
            return ExitStatus::Rejected;
        }
    };
    if chosen.is_empty() {
        println!("nothing is running");
        return ExitStatus::Ok;
    }
    let mut status = ExitStatus::Ok;
    for run in chosen {
        match client.cancel_run(&run.run_id).await {
            Ok(_) => println!("cancelling {} ({})", run.run_id, run.state),
            Err(error) => status = report(&error),
        }
    }
    status
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
    use jarvis_core::{RunOutcome, RunState, UtcTimestamp};

    fn run(id: &str, outcome: Option<RunOutcome>) -> RunSummaryReply {
        RunSummaryReply {
            run_id: id.to_owned(),
            session_id: "s".to_owned(),
            objective: "o".to_owned(),
            state: if outcome.is_some() {
                RunState::Completed
            } else {
                RunState::Executing
            },
            outcome,
            error_code: None,
            started_at: UtcTimestamp::now(&jarvis_core::SystemClock),
            completed_at: None,
            answer: None,
        }
    }

    #[test]
    fn all_selects_only_unfinished_runs() {
        let runs = [
            run("aa1", None),
            run("bb2", Some(RunOutcome::Succeeded)),
            run("cc3", None),
        ];
        let chosen = choose(&runs, None, true).unwrap_or_default();
        let ids: Vec<&str> = chosen.iter().map(|run| run.run_id.as_str()).collect();
        assert_eq!(ids, ["aa1", "cc3"]);
    }

    #[test]
    fn a_prefix_names_one_unfinished_run_and_refuses_the_rest() {
        let runs = [
            run("aa1", None),
            run("aa2", None),
            run("bb2", Some(RunOutcome::Succeeded)),
        ];
        assert_eq!(
            choose(&runs, Some("aa1"), false).map(|found| found.len()),
            Ok(1)
        );
        assert!(
            choose(&runs, Some("aa"), false).is_err(),
            "an ambiguous prefix must not guess"
        );
        assert!(choose(&runs, Some("zz"), false).is_err());
        let finished = choose(&runs, Some("bb"), false).err().unwrap_or_default();
        assert!(finished.contains("already finished"), "{finished}");
        assert!(
            choose(&runs, None, false).is_err(),
            "no target and no --all is a usage error"
        );
    }
}
