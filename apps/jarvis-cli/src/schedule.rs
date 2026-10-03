//! The `jarvis schedule` and `jarvis runs` verbs: tasks that run while you are away, and what they said.
//!
//! # What a schedule is, in one sentence
//!
//! A request to start an ordinary run later — so it has exactly the authority an ordinary run has, which means an
//! unattended task that wants to fetch a page or run code **stops and waits for you**, and `jarvis approvals`
//! (or the next `jarvis ask`) is where you answer. Nothing here can approve anything.
//!
//! # Cadences, and the one that is missing
//!
//! `--every 30m|6h|2d` and `--at 2026-10-04T09:00:00Z` mean the same thing on every machine. "Every day at 08:00
//! local time" needs time zones and daylight-saving rules (`P6-003`), and a half-right version would fire an hour
//! late for half the year without telling anyone, so it does not exist yet. `--every 24h` is the honest form.

use jarvis_core::UtcTimestamp;
use jarvis_protocol::{CreateScheduleRequest, RunListReply, ScheduleReply};

use crate::api_client::{ApiClient, ApiError};
use crate::output::ExitStatus;

/// Runs one schedule verb.
pub async fn run_schedule(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let json = crate::json_requested(arguments);
    match arguments.get(1).map(String::as_str) {
        Some("add") => add(client, arguments).await,
        Some("list") | None => list(client, json).await,
        Some("pause") => toggle(client, arguments, false).await,
        Some("resume") => toggle(client, arguments, true).await,
        Some("remove") => remove(client, arguments).await,
        Some(other) => {
            eprintln!("jarvis: unknown schedule command {other:?}");
            eprintln!("{}", schedule_usage());
            ExitStatus::Usage
        }
    }
}

/// The usage text for `jarvis schedule`.
pub(crate) const fn schedule_usage() -> &'static str {
    "usage: jarvis schedule <add|list|pause|resume|remove> [...]\n       jarvis schedule add <objective...> (--every 30m|6h|2d | --at 2026-10-04T09:00:00Z)\n       jarvis schedule list [--json]\n       jarvis schedule pause|resume|remove ID"
}

/// Splits `add` arguments into the objective words and the cadence flags.
///
/// Flags are taken from anywhere in the line so an unquoted objective works, and a flag's value is never mistaken
/// for an objective word.
fn parse_add(arguments: &[String]) -> Result<CreateScheduleRequest, String> {
    let mut words: Vec<&str> = Vec::new();
    let mut every: Option<String> = None;
    let mut at: Option<UtcTimestamp> = None;
    let mut index = 2;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--every" => {
                every = Some(
                    arguments
                        .get(index + 1)
                        .ok_or("--every needs an interval such as 30m, 6h or 2d")?
                        .clone(),
                );
                index += 2;
            }
            "--at" => {
                let text = arguments
                    .get(index + 1)
                    .ok_or("--at needs a time such as 2026-10-04T09:00:00Z")?;
                at = Some(text.parse::<UtcTimestamp>().map_err(|_| {
                    format!("{text:?} is not an RFC 3339 time such as 2026-10-04T09:00:00Z")
                })?);
                index += 2;
            }
            "--root" => index += 2,
            "--json" => index += 1,
            word => {
                words.push(word);
                index += 1;
            }
        }
    }
    if words.is_empty() {
        return Err("schedule add needs an objective, for example: jarvis schedule add check my calendar --every 1d".to_owned());
    }
    Ok(CreateScheduleRequest {
        objective: words.join(" "),
        every,
        at,
    })
}

async fn add(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let request = match parse_add(arguments) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("jarvis: {message}");
            eprintln!("{}", schedule_usage());
            return ExitStatus::Usage;
        }
    };
    match client.create_schedule(&request).await {
        Ok(created) => {
            println!("scheduled {}", created.schedule_id);
            println!("  {}", describe(&created));
            eprintln!(
                "jarvis: it runs while you are away; anything it needs approval for waits in `jarvis approvals list`"
            );
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}

async fn list(client: &ApiClient, json: bool) -> ExitStatus {
    let reply = match client.list_schedules().await {
        Ok(reply) => reply,
        Err(error) => return report(&error),
    };
    if json {
        return print_json(&reply);
    }
    if reply.schedules.is_empty() {
        println!("nothing is scheduled");
        return ExitStatus::Ok;
    }
    println!("{} scheduled task(s)", reply.total);
    for schedule in &reply.schedules {
        println!("  {}  {}", schedule.schedule_id, describe(schedule));
        println!("      {}", schedule.objective);
        let mut history = vec![format!("ran {} time(s)", schedule.fire_count)];
        if schedule.skipped_count > 0 {
            history.push(format!(
                "skipped {} because the previous run was still waiting",
                schedule.skipped_count
            ));
        }
        if let Some(last) = &schedule.last_run_id {
            history.push(format!("last run {last}"));
        }
        println!("      {}", history.join(", "));
    }
    ExitStatus::Ok
}

/// Describes when a task fires, in the words a person would use.
fn describe(schedule: &ScheduleReply) -> String {
    let cadence = match (schedule.cadence.as_str(), schedule.interval_seconds) {
        ("every", Some(seconds)) => format!("every {}", humanize(seconds)),
        _ => "once".to_owned(),
    };
    match schedule.next_run_at {
        Some(next) => format!("{cadence}, next at {next}"),
        None if schedule.fire_count > 0 && schedule.cadence == "once" => format!("{cadence}, done"),
        None => format!("{cadence}, paused"),
    }
}

fn humanize(seconds: u64) -> String {
    for (unit, size) in [("d", 86_400), ("h", 3600), ("m", 60)] {
        if seconds.is_multiple_of(size) {
            return format!("{}{unit}", seconds / size);
        }
    }
    format!("{seconds}s")
}

/// Resolves what the person typed to a task: the whole identifier, or a prefix that names exactly one.
async fn resolve(client: &ApiClient, typed: &str) -> Result<String, ExitStatus> {
    let reply = client
        .list_schedules()
        .await
        .map_err(|error| report(&error))?;
    let matches: Vec<&ScheduleReply> = reply
        .schedules
        .iter()
        .filter(|schedule| schedule.schedule_id.starts_with(typed))
        .collect();
    match matches.as_slice() {
        [only] => Ok(only.schedule_id.clone()),
        [] => {
            eprintln!(
                "jarvis: no scheduled task starts with {typed:?} (see `jarvis schedule list`)"
            );
            Err(ExitStatus::Rejected)
        }
        _ => {
            eprintln!("jarvis: {typed:?} matches more than one task; type more of the identifier");
            Err(ExitStatus::Rejected)
        }
    }
}

fn identifier(arguments: &[String]) -> Option<&String> {
    arguments.get(2).filter(|value| !value.starts_with("--"))
}

async fn toggle(client: &ApiClient, arguments: &[String], enabled: bool) -> ExitStatus {
    let Some(typed) = identifier(arguments) else {
        eprintln!("{}", schedule_usage());
        return ExitStatus::Usage;
    };
    let id = match resolve(client, typed).await {
        Ok(id) => id,
        Err(status) => return status,
    };
    match client.set_schedule_enabled(&id, enabled).await {
        Ok(schedule) => {
            println!("{}  {}", schedule.schedule_id, describe(&schedule));
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}

async fn remove(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let Some(typed) = identifier(arguments) else {
        eprintln!("{}", schedule_usage());
        return ExitStatus::Usage;
    };
    let id = match resolve(client, typed).await {
        Ok(id) => id,
        Err(status) => return status,
    };
    match client.remove_schedule(&id).await {
        Ok(()) => {
            println!("removed {id}");
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}

/// Runs one `jarvis runs` verb.
pub async fn run_runs(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let json = crate::json_requested(arguments);
    let full = arguments.iter().any(|argument| argument == "--full");
    let limit = arguments
        .windows(2)
        .find(|pair| pair[0] == "--limit")
        .and_then(|pair| pair[1].parse::<u32>().ok())
        .unwrap_or(10);
    match arguments.get(1).map(String::as_str) {
        Some("list") | None => {}
        Some(other) if other.starts_with("--") => {}
        Some(other) => {
            eprintln!("jarvis: unknown runs command {other:?}");
            eprintln!("usage: jarvis runs [list] [--limit N] [--full] [--json]");
            return ExitStatus::Usage;
        }
    }
    let reply = match client.list_runs(limit).await {
        Ok(reply) => reply,
        Err(error) => return report(&error),
    };
    if json {
        return print_json(&reply);
    }
    render_runs(&reply, full);
    ExitStatus::Ok
}

fn render_runs(reply: &RunListReply, full: bool) {
    if reply.runs.is_empty() {
        println!("no runs yet");
        return;
    }
    for run in &reply.runs {
        let outcome = run.outcome.map_or_else(
            || run.state.to_string(),
            |outcome| outcome.as_str().to_owned(),
        );
        println!("{}  {}  {}", run.run_id, outcome, run.started_at);
        println!("  {}", one_line(&run.objective, 100));
        match &run.answer {
            Some(answer) if full => {
                for line in answer.lines() {
                    println!("  | {line}");
                }
            }
            Some(answer) => println!("  | {}", one_line(answer, 200)),
            None if run.state.as_str() == "awaiting_approval" => {
                println!("  (waiting for you: `jarvis approvals list`)");
            }
            None => {}
        }
    }
}

/// Collapses text to one line of at most `limit` characters, marking a cut.
fn one_line(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= limit {
        return flat;
    }
    let cut: String = flat.chars().take(limit).collect();
    format!("{cut}…")
}

fn print_json<T: serde::Serialize>(value: &T) -> ExitStatus {
    match serde_json::to_string_pretty(value) {
        Ok(rendered) => {
            println!("{rendered}");
            ExitStatus::Ok
        }
        Err(error) => {
            eprintln!("jarvis: the reply could not be rendered: {error}");
            ExitStatus::Internal
        }
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

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn an_unquoted_objective_and_its_flags_are_separated() {
        let request = parse_add(&words("schedule add check my calendar --every 1d"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(request.objective, "check my calendar");
        assert_eq!(request.every.as_deref(), Some("1d"));
        assert!(request.at.is_none());
    }

    #[test]
    fn the_flag_may_come_first_and_its_value_is_not_an_objective_word() {
        let request = parse_add(&words("schedule add --every 6h summarise the news"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(request.objective, "summarise the news");
        assert_eq!(request.every.as_deref(), Some("6h"));
    }

    #[test]
    fn a_one_off_time_is_parsed_and_a_bad_one_is_named() {
        let request = parse_add(&words("schedule add remind me --at 2099-01-01T00:00:00Z"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(request.at.is_some());
        let error = parse_add(&words("schedule add remind me --at tomorrow"));
        assert!(error.is_err_and(|message| message.contains("RFC 3339")));
    }

    #[test]
    fn a_missing_objective_or_flag_value_is_refused() {
        assert!(parse_add(&words("schedule add --every 1h")).is_err());
        assert!(parse_add(&words("schedule add check --every")).is_err());
    }

    #[test]
    fn intervals_are_described_in_the_largest_unit_that_divides_them() {
        assert_eq!(humanize(86_400), "1d");
        assert_eq!(humanize(21_600), "6h");
        assert_eq!(humanize(1800), "30m");
        assert_eq!(humanize(90), "90s");
    }

    #[test]
    fn a_long_answer_is_cut_to_one_marked_line() {
        let long = "word ".repeat(100);
        let line = one_line(&long, 30);
        assert!(line.ends_with('…'));
        assert!(line.chars().count() <= 31);
        assert_eq!(one_line("a\n  b", 10), "a b");
    }
}
