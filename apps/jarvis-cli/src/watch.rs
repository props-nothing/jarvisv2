//! `jarvis watch`: what the assistant is doing right now, on one screen.
//!
//! ```text
//! jarvis watch              # refreshes every 2 seconds until Ctrl+C
//! jarvis watch --once       # prints once (also what happens when output is not a terminal)
//! jarvis watch --interval 5
//! ```
//!
//! A transcript shows what was said; this shows the **work**: what is waiting for you, what is running (including
//! sub-agents and scheduled tasks), what is scheduled next, and what just finished. It reads three existing
//! endpoints and invents no state of its own, so it cannot disagree with `jarvis runs`, `jarvis approvals` and
//! `jarvis schedule`.
//!
//! The screen is plain ASCII on purpose: it has to render in any terminal, over SSH, and in a log.

use std::fmt::Write as _;
use std::io::IsTerminal;
use std::time::Duration;

use jarvis_core::{SystemClock, UtcTimestamp};
use jarvis_protocol::{ApprovalListReply, RunListReply, ScheduleListReply};

use crate::api_client::{ApiClient, ApiError};
use crate::output::ExitStatus;

/// How many recent runs are read. The list endpoint's own maximum.
const RUN_LIMIT: u32 = 50;

/// How many finished runs the screen keeps.
const RECENT_SHOWN: usize = 5;

/// Characters of an objective or answer shown on one line.
const LINE_CHARS: usize = 90;

const DEFAULT_INTERVAL_SECONDS: u64 = 2;

/// Runs `jarvis watch`.
pub async fn run_watch(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let once =
        arguments.iter().any(|argument| argument == "--once") || !std::io::stdout().is_terminal();
    let interval = arguments
        .windows(2)
        .find(|pair| pair[0] == "--interval")
        .and_then(|pair| pair[1].parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECONDS)
        .clamp(1, 60);
    loop {
        let screen = match snapshot(client).await {
            Ok((runs, approvals, schedules)) => render(
                &runs,
                &approvals,
                &schedules,
                UtcTimestamp::now(&SystemClock).unix_nanos(),
            ),
            Err(error) => return report(&error),
        };
        if once {
            print!("{screen}");
            return ExitStatus::Ok;
        }
        clear();
        print!("{screen}");
        println!("\n(refreshing every {interval}s; Ctrl+C to leave)");
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

async fn snapshot(
    client: &ApiClient,
) -> Result<(RunListReply, ApprovalListReply, ScheduleListReply), ApiError> {
    Ok((
        client.list_runs(RUN_LIMIT).await?,
        client.list_approvals().await?,
        client.list_schedules().await?,
    ))
}

/// Clears the screen the way the platform's console understands. The classic Windows console does not interpret
/// escape sequences unless asked to, so it is asked with its own command instead.
fn clear() {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("cmd")
            .args(["/C", "cls"])
            .status();
    }
    #[cfg(not(windows))]
    {
        print!("\x1b[2J\x1b[H");
    }
}

/// Builds the screen. Pure over its inputs and the clock reading, so it is tested without a daemon.
#[must_use]
pub fn render(
    runs: &RunListReply,
    approvals: &ApprovalListReply,
    schedules: &ScheduleListReply,
    now_nanos: i128,
) -> String {
    let working: Vec<_> = runs
        .runs
        .iter()
        .filter(|run| run.outcome.is_none() && run.state.as_str() != "awaiting_approval")
        .collect();
    let waiting: Vec<_> = runs
        .runs
        .iter()
        .filter(|run| run.state.as_str() == "awaiting_approval")
        .collect();
    let finished: Vec<_> = runs
        .runs
        .iter()
        .filter(|run| run.outcome.is_some())
        .take(RECENT_SHOWN)
        .collect();
    let upcoming: Vec<_> = schedules
        .schedules
        .iter()
        .filter(|schedule| schedule.enabled)
        .collect();

    let mut screen = String::new();
    let _ = writeln!(
        screen,
        "JARVIS   {} working   {} waiting for you   {} scheduled",
        working.len(),
        approvals.approvals.len().max(waiting.len()),
        upcoming.len()
    );

    if !approvals.approvals.is_empty() {
        screen.push_str("\nWAITING FOR YOU\n");
        for approval in &approvals.approvals {
            let _ = writeln!(
                screen,
                "  [!] {}  risk {}  {}",
                approval.tool,
                approval.risk_level,
                // What it would do, not the generic preview: the arguments are what a person decides on.
                approval.arguments.as_ref().map_or_else(
                    || clip(&approval.preview, LINE_CHARS),
                    |arguments| clip(&arguments.to_string(), LINE_CHARS)
                )
            );
            let _ = writeln!(
                screen,
                "      jarvis approvals approve {}   |   jarvis approvals deny {}",
                approval.approval_id, approval.approval_id
            );
        }
    }

    if !working.is_empty() {
        screen.push_str("\nWORKING\n");
        for run in &working {
            let _ = writeln!(
                screen,
                "  [~] {}  {}  {}\n      {}",
                run.run_id,
                run.state,
                age(now_nanos, run.started_at.unix_nanos()),
                clip(&run.objective, LINE_CHARS)
            );
        }
        screen.push_str("      stop one: jarvis cancel RUN   |   stop all: jarvis cancel --all\n");
    }

    if !upcoming.is_empty() {
        screen.push_str("\nSCHEDULED\n");
        for schedule in &upcoming {
            let next = schedule
                .next_run_at
                .map_or_else(|| "-".to_owned(), |at| until(now_nanos, at.unix_nanos()));
            let _ = writeln!(
                screen,
                "  next {next:>8}  {}  {}",
                schedule.cadence,
                clip(&schedule.objective, LINE_CHARS)
            );
        }
    }

    if !finished.is_empty() {
        screen.push_str("\nRECENT\n");
        for run in &finished {
            let mark = match run.outcome.map(jarvis_core::RunOutcome::as_str) {
                Some("succeeded") => "ok",
                Some("cancelled") => "--",
                _ => "xx",
            };
            let _ = writeln!(
                screen,
                "  [{mark}] {}  {}",
                clip(&run.objective, 50),
                run.answer
                    .as_deref()
                    .map_or_else(String::new, |answer| format!("=> {}", clip(answer, 60)))
            );
        }
    }

    if working.is_empty() && approvals.approvals.is_empty() && waiting.is_empty() {
        screen.push_str("\nIdle. Ask something with `jarvis ask` or `jarvis chat`.\n");
    }
    screen
}

/// Collapses text to one line of at most `limit` characters, marking a cut.
fn clip(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let cut: String = flat.chars().take(limit).collect();
    format!("{cut}...")
}

/// How long ago, as `4s`, `3m`, `2h`.
fn age(now_nanos: i128, then_nanos: i128) -> String {
    span((now_nanos - then_nanos).max(0))
}

/// How long until, as `in 4s`; `due` when it is past.
fn until(now_nanos: i128, then_nanos: i128) -> String {
    if then_nanos <= now_nanos {
        return "due".to_owned();
    }
    format!("in {}", span(then_nanos - now_nanos))
}

fn span(nanos: i128) -> String {
    let seconds = nanos / 1_000_000_000;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
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

    #[test]
    fn spans_read_as_the_largest_whole_unit() {
        assert_eq!(span(4 * 1_000_000_000), "4s");
        assert_eq!(span(190 * 1_000_000_000), "3m");
        assert_eq!(span(7300 * 1_000_000_000), "2h");
        assert_eq!(until(10, 5), "due");
        assert_eq!(until(0, 61 * 1_000_000_000), "in 1m");
    }

    #[test]
    fn long_text_is_one_line_and_marked_when_cut() {
        assert_eq!(clip("a\n  b   c", 20), "a b c");
        assert_eq!(clip(&"x".repeat(30), 10), format!("{}...", "x".repeat(10)));
    }

    #[test]
    fn an_idle_daemon_says_so_and_names_the_next_step() {
        let screen = render(
            &RunListReply {
                total: 0,
                runs: Vec::new(),
            },
            &ApprovalListReply {
                total: 0,
                approvals: Vec::new(),
            },
            &ScheduleListReply {
                total: 0,
                schedules: Vec::new(),
            },
            0,
        );
        assert!(
            screen.contains("0 working   0 waiting for you   0 scheduled"),
            "{screen}"
        );
        assert!(screen.contains("Idle."), "{screen}");
    }
    fn run(
        id: &str,
        objective: &str,
        state: jarvis_core::RunState,
        outcome: Option<jarvis_core::RunOutcome>,
    ) -> jarvis_protocol::RunSummaryReply {
        jarvis_protocol::RunSummaryReply {
            run_id: id.to_owned(),
            session_id: "s".to_owned(),
            objective: objective.to_owned(),
            state,
            outcome,
            error_code: None,
            started_at: UtcTimestamp::now(&SystemClock),
            completed_at: None,
            answer: outcome.map(|_| "forty-two".to_owned()),
        }
    }

    #[test]
    fn the_screen_separates_what_is_running_from_what_is_waiting_and_what_is_done() {
        let runs = RunListReply {
            total: 3,
            runs: vec![
                run(
                    "run-working",
                    "[sub-agent] read the pages",
                    jarvis_core::RunState::Executing,
                    None,
                ),
                run(
                    "run-parked",
                    "run a snippet",
                    jarvis_core::RunState::AwaitingApproval,
                    None,
                ),
                run(
                    "run-done",
                    "what is 6*7",
                    jarvis_core::RunState::Completed,
                    Some(jarvis_core::RunOutcome::Succeeded),
                ),
            ],
        };
        let approvals = ApprovalListReply {
            total: 1,
            approvals: vec![jarvis_protocol::PendingApprovalReply {
                approval_id: "appr-1".to_owned(),
                state: "pending".to_owned(),
                run_id: "run-parked".to_owned(),
                call_id: "call".to_owned(),
                tool: "jarvis.code.run".to_owned(),
                tool_version: "1.0.0".to_owned(),
                risk_level: 3,
                required_strength: "credential".to_owned(),
                preview: "run code".to_owned(),
                arguments: None,
                created_at: UtcTimestamp::now(&SystemClock),
                expires_at: UtcTimestamp::now(&SystemClock),
            }],
        };
        let screen = render(
            &runs,
            &approvals,
            &ScheduleListReply {
                total: 0,
                schedules: Vec::new(),
            },
            UtcTimestamp::now(&SystemClock).unix_nanos(),
        );
        assert!(screen.contains("1 working   1 waiting for you"), "{screen}");
        let waiting = screen.find("WAITING FOR YOU").unwrap_or(usize::MAX);
        let working = screen.find("WORKING").unwrap_or(usize::MAX);
        let recent = screen.find("RECENT").unwrap_or(usize::MAX);
        assert!(waiting < working && working < recent, "{screen}");
        assert!(
            screen.contains("jarvis approvals approve appr-1"),
            "{screen}"
        );
        assert!(screen.contains("[sub-agent] read the pages"), "{screen}");
        assert!(screen.contains("=> forty-two"), "{screen}");
        assert!(
            !screen.contains("Idle."),
            "a busy daemon is not idle: {screen}"
        );
    }
}
