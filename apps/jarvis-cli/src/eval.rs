//! `jarvis eval`: run a suite of prompts against the live daemon and score the answers (`ADR-0166`).
//!
//! Correctness is asserted by the acceptance gates; this is the other question, **is the assistant getting better or worse at the
//! work it is for**. A suite is a TOML file of prompts with expectations that can be checked without a second model: words the answer
//! must or must not contain, the tools it must or must not use, how long it may take and how many tokens it may spend. Each case is an
//! ordinary run in a fresh conversation, so it goes through the same policy, approvals and tools as a person's own message. A case
//! that needs an approval is parked, counted, and cancelled: a suite should hold read-only work.
//!
//! Every run is saved under the data directory and compared with the previous run of the same suite, which is what makes a change to
//! a prompt, a tool or a model a measurement instead of a guess.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use jarvis_core::{RunState, SystemClock, UtcTimestamp};
use jarvis_storage::AppPaths;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::api_client::ApiClient;
use crate::output::ExitStatus;
use crate::schedule::print_json;

const USAGE: &str = "usage: jarvis eval run SUITE.toml [--case ID]... [--json]    # run a suite against the daemon and score it\n       jarvis eval history [NAME]                                  # the saved runs of a suite (or all suites)";

/// How long a case may take unless it says otherwise.
const DEFAULT_SECONDS: u64 = 180;
/// The most seconds a case may be given.
const MAX_SECONDS: u64 = 1800;
/// How often a running case is looked at.
const POLL: Duration = Duration::from_millis(700);
/// The most cases in one suite.
const MAX_CASES: usize = 200;
/// The longest prompt, in characters (a run objective holds 4,096).
const MAX_PROMPT_CHARS: usize = 4000;
/// How much of an answer a saved record keeps.
const ANSWER_HEAD_CHARS: usize = 500;
/// The most event pages read for one run.
const MAX_EVENT_PAGES: u32 = 20;

/// A suite of cases.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Suite {
    /// Names the suite and its saved history: letters, digits, `-` and `_`.
    pub name: String,
    /// What the suite is for.
    #[serde(default)]
    pub description: String,
    /// The cases, run in order.
    #[serde(rename = "case")]
    pub cases: Vec<Case>,
}

/// One prompt and what must be true of the run it starts.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Case {
    /// Names the case in the results.
    pub id: String,
    /// The message sent, exactly as a person would send it.
    pub prompt: String,
    /// What must hold.
    #[serde(default)]
    pub expect: Expect,
}

/// What must be true of a run. Every field is optional; a case with none only has to complete.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct Expect {
    /// Every one of these must be in the answer (any capitalisation).
    pub contains: Vec<String>,
    /// At least one of these must be in the answer.
    pub contains_any: Vec<String>,
    /// None of these may be in the answer.
    pub not_contains: Vec<String>,
    /// Each of these tools must have been requested. `web.fetch` and `jarvis.web.fetch` mean the same.
    pub tools_used: Vec<String>,
    /// None of these tools may have been requested.
    pub tools_not_used: Vec<String>,
    /// The most tool calls the run may make.
    pub max_tool_calls: Option<u32>,
    /// The most input tokens the run may use.
    pub max_input_tokens: Option<u64>,
    /// The longest the answer may be, in characters.
    pub max_answer_chars: Option<usize>,
    /// The longest the run may take; a run past it is stopped and fails.
    pub max_seconds: Option<u64>,
    /// The run is expected to stop for an approval (it is cancelled once it does), instead of completing.
    pub parks_for_approval: bool,
}

/// How a run ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Finish {
    /// It answered.
    Completed,
    /// It failed, with the daemon's reason.
    Failed(String),
    /// It stopped for an approval.
    Parked,
    /// It did not finish in time and was cancelled.
    TimedOut,
    /// It was cancelled by someone else.
    Cancelled,
}

/// What a run did.
#[derive(Clone, Debug)]
pub(crate) struct Observed {
    pub finish: Finish,
    pub answer: Option<String>,
    pub tools: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub seconds: u64,
}

/// One check and whether it held.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Check {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

fn check(name: impl Into<String>, passed: bool, detail: impl Into<String>) -> Check {
    Check {
        name: name.into(),
        passed,
        detail: detail.into(),
    }
}

/// Whether a requested tool is the one an expectation names (`web.fetch` matches `jarvis.web.fetch`).
pub(crate) fn tool_matches(seen: &str, wanted: &str) -> bool {
    seen == wanted || seen.strip_prefix("jarvis.") == Some(wanted)
}

/// Scores one run against what was expected of it. Pure: nothing here reads a clock or the daemon.
pub(crate) fn evaluate(expect: &Expect, seen: &Observed) -> Vec<Check> {
    let mut checks = Vec::new();
    let ended_as_expected = if expect.parks_for_approval {
        seen.finish == Finish::Parked
    } else {
        seen.finish == Finish::Completed
    };
    checks.push(check(
        if expect.parks_for_approval {
            "stops for approval"
        } else {
            "completes"
        },
        ended_as_expected,
        format!("{:?}", seen.finish),
    ));
    let answer = seen.answer.as_deref().unwrap_or("");
    let lower = answer.to_lowercase();
    for term in &expect.contains {
        let held = lower.contains(&term.to_lowercase());
        checks.push(check(
            format!("contains \"{term}\""),
            held,
            if held { "" } else { "not in the answer" },
        ));
    }
    if !expect.contains_any.is_empty() {
        let held = expect
            .contains_any
            .iter()
            .any(|term| lower.contains(&term.to_lowercase()));
        checks.push(check(
            format!("contains one of {:?}", expect.contains_any),
            held,
            if held { "" } else { "none is in the answer" },
        ));
    }
    for term in &expect.not_contains {
        let absent = !lower.contains(&term.to_lowercase());
        checks.push(check(
            format!("does not contain \"{term}\""),
            absent,
            if absent { "" } else { "it is in the answer" },
        ));
    }
    for wanted in &expect.tools_used {
        let used = seen.tools.iter().any(|tool| tool_matches(tool, wanted));
        checks.push(check(
            format!("uses {wanted}"),
            used,
            if used {
                String::new()
            } else {
                format!("it requested {:?}", seen.tools)
            },
        ));
    }
    for banned in &expect.tools_not_used {
        let unused = !seen.tools.iter().any(|tool| tool_matches(tool, banned));
        checks.push(check(
            format!("does not use {banned}"),
            unused,
            if unused { "" } else { "it did" },
        ));
    }
    push_limits(expect, seen, &mut checks);
    checks
}

fn push_limits(expect: &Expect, seen: &Observed, checks: &mut Vec<Check>) {
    if let Some(limit) = expect.max_tool_calls {
        let calls = u32::try_from(seen.tools.len()).unwrap_or(u32::MAX);
        checks.push(check(
            format!("at most {limit} tool calls"),
            calls <= limit,
            format!("{calls} calls"),
        ));
    }
    if let Some(limit) = expect.max_input_tokens {
        checks.push(check(
            format!("at most {limit} input tokens"),
            seen.input_tokens <= limit,
            format!("{} tokens", seen.input_tokens),
        ));
    }
    if let Some(limit) = expect.max_answer_chars {
        let length = seen
            .answer
            .as_deref()
            .map_or(0, |text| text.chars().count());
        checks.push(check(
            format!("answer at most {limit} characters"),
            length <= limit,
            format!("{length} characters"),
        ));
    }
    if let Some(limit) = expect.max_seconds {
        checks.push(check(
            format!("within {limit} s"),
            seen.seconds <= limit && seen.finish != Finish::TimedOut,
            format!("{} s", seen.seconds),
        ));
    }
}

/// An event as the scorer reads it, so the reading can be tested without a daemon.
pub(crate) struct Event<'a> {
    pub kind: &'a str,
    pub summary: Option<&'a str>,
    pub payload: &'a Value,
}

/// What the events of a run say it did: the answer, the tools it requested, and the tokens its model calls used.
pub(crate) fn summarise(events: &[Event<'_>]) -> (Option<String>, Vec<String>, u64, u64) {
    let mut answer = None;
    let mut tools = Vec::new();
    let (mut input, mut output) = (0_u64, 0_u64);
    for event in events {
        match event.kind {
            "tool_requested" => {
                let name = event
                    .summary
                    .and_then(|summary| summary.strip_prefix("Requested "))
                    .or_else(|| event.payload.get("tool").and_then(Value::as_str));
                if let Some(name) = name {
                    tools.push(name.to_owned());
                }
            }
            "usage_updated" => {
                let count = |key: &str| event.payload.get(key).and_then(Value::as_u64).unwrap_or(0);
                input = input.saturating_add(count("input_tokens"));
                output = output.saturating_add(count("output_tokens"));
            }
            "output_completed" => {
                if let Some(text) = event.payload.get("text").and_then(Value::as_str) {
                    answer = Some(text.to_owned());
                }
            }
            _ => {}
        }
    }
    (answer, tools, input, output)
}

/// One case as saved.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct CaseRecord {
    pub id: String,
    pub passed: bool,
    pub seconds: u64,
    pub tools: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub failures: Vec<String>,
    pub answer_head: String,
}

/// One run of a suite as saved.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct RunRecord {
    pub suite: String,
    pub model: String,
    pub started_at: String,
    pub cases: Vec<CaseRecord>,
}

impl RunRecord {
    fn passed(&self) -> usize {
        self.cases.iter().filter(|case| case.passed).count()
    }

    fn input_tokens(&self) -> u64 {
        self.cases.iter().map(|case| case.input_tokens).sum()
    }

    fn output_tokens(&self) -> u64 {
        self.cases.iter().map(|case| case.output_tokens).sum()
    }

    fn seconds(&self) -> u64 {
        self.cases.iter().map(|case| case.seconds).sum()
    }
}

/// What changed between two runs of the same suite.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Comparison {
    /// Passed before, fails now.
    pub regressions: Vec<String>,
    /// Failed before, passes now.
    pub fixed: Vec<String>,
    /// Not in the earlier run.
    pub new_cases: Vec<String>,
}

/// Compares a run with an earlier one, by case id.
pub(crate) fn compare(before: &RunRecord, now: &RunRecord) -> Comparison {
    let mut comparison = Comparison::default();
    for case in &now.cases {
        match before.cases.iter().find(|earlier| earlier.id == case.id) {
            None => comparison.new_cases.push(case.id.clone()),
            Some(earlier) if earlier.passed && !case.passed => {
                comparison.regressions.push(case.id.clone());
            }
            Some(earlier) if !earlier.passed && case.passed => {
                comparison.fixed.push(case.id.clone());
            }
            Some(_) => {}
        }
    }
    comparison
}

/// Reads and validates a suite file's text.
///
/// # Errors
///
/// A sentence saying what is wrong with the suite.
pub(crate) fn parse_suite(text: &str) -> Result<Suite, String> {
    let suite: Suite =
        toml::from_str(text).map_err(|error| format!("the suite could not be read: {error}"))?;
    let name_ok = !suite.name.is_empty()
        && suite.name.len() <= 60
        && suite
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !name_ok {
        return Err(
            "the suite name must be 1 to 60 letters, digits, - or _ (it names the saved history)"
                .to_owned(),
        );
    }
    if suite.cases.is_empty() || suite.cases.len() > MAX_CASES {
        return Err(format!("a suite needs 1 to {MAX_CASES} cases"));
    }
    let mut seen = std::collections::HashSet::new();
    for case in &suite.cases {
        if case.id.is_empty() || case.id.len() > 60 || case.id.chars().any(char::is_control) {
            return Err("a case id must be 1 to 60 characters".to_owned());
        }
        if !seen.insert(case.id.as_str()) {
            return Err(format!("the case id {:?} is used twice", case.id));
        }
        let length = case.prompt.chars().count();
        if length == 0 || length > MAX_PROMPT_CHARS {
            return Err(format!(
                "the prompt of {:?} must be 1 to {MAX_PROMPT_CHARS} characters",
                case.id
            ));
        }
        if case
            .expect
            .max_seconds
            .is_some_and(|s| s == 0 || s > MAX_SECONDS)
        {
            return Err(format!(
                "max_seconds of {:?} must be 1 to {MAX_SECONDS}",
                case.id
            ));
        }
    }
    Ok(suite)
}

/// Runs one case against the daemon and describes what happened.
async fn observe(client: &ApiClient, case: &Case) -> Observed {
    let limit = Duration::from_secs(case.expect.max_seconds.unwrap_or(DEFAULT_SECONDS));
    let started = Instant::now();
    let failed = |reason: String| Observed {
        finish: Finish::Failed(reason),
        answer: None,
        tools: Vec::new(),
        input_tokens: 0,
        output_tokens: 0,
        seconds: started.elapsed().as_secs(),
    };
    let run = match client.start_run_in_session(&case.prompt, None).await {
        Ok(run) => run,
        Err(error) => return failed(format!("the run could not be started: {error}")),
    };
    let finish = loop {
        match client.read_run(&run.run_id).await {
            Ok(now) => match now.state {
                RunState::Completed => break Finish::Completed,
                RunState::Failed => {
                    break Finish::Failed(now.error_code.unwrap_or_else(|| "failed".to_owned()));
                }
                RunState::Cancelled => break Finish::Cancelled,
                RunState::AwaitingApproval => break Finish::Parked,
                _ => {}
            },
            Err(error) => break Finish::Failed(format!("the run could not be read: {error}")),
        }
        if started.elapsed() >= limit {
            break Finish::TimedOut;
        }
        tokio::time::sleep(POLL).await;
    };
    let seconds = started.elapsed().as_secs();
    // A run left parked or running would stay on the owner's screen after the suite is over.
    if matches!(finish, Finish::Parked | Finish::TimedOut) {
        let _ = client.cancel_run(&run.run_id).await;
    }
    let (answer, tools, input_tokens, output_tokens) = read_events(client, &run.run_id).await;
    Observed {
        finish,
        answer,
        tools,
        input_tokens,
        output_tokens,
        seconds,
    }
}

/// Reads a run's events, page by page, and summarises them. An unreadable run summarises as empty.
async fn read_events(client: &ApiClient, run_id: &str) -> (Option<String>, Vec<String>, u64, u64) {
    let mut collected = Vec::new();
    let mut from = 1_u32;
    for _ in 0..MAX_EVENT_PAGES {
        let Ok(page) = client.read_run_events(run_id, from, 1000).await else {
            break;
        };
        let count = page.events.len();
        let next = page
            .events
            .last()
            .map(|event| event.sequence.get().saturating_add(1));
        collected.extend(page.events);
        match next {
            Some(next) if count >= 1000 => from = next,
            _ => break,
        }
    }
    let events: Vec<Event<'_>> = collected
        .iter()
        .map(|event| Event {
            kind: event.kind.as_str(),
            summary: event.summary.as_deref(),
            payload: &event.payload,
        })
        .collect();
    summarise(&events)
}

fn thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn head(text: &str, limit: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(limit).collect()
}

fn history_dir(paths: &AppPaths, suite: &str) -> PathBuf {
    paths.data().join("evals").join(suite)
}

fn save(paths: &AppPaths, record: &RunRecord) -> Result<PathBuf, String> {
    let directory = history_dir(paths, &record.suite);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("the history folder could not be made: {error}"))?;
    let stamp: String = record
        .started_at
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let path = directory.join(format!("{stamp}.json"));
    let text = serde_json::to_string_pretty(record).map_err(|error| error.to_string())?;
    std::fs::write(&path, text)
        .map_err(|error| format!("the result could not be saved: {error}"))?;
    Ok(path)
}

fn saved_runs(directory: &Path) -> Vec<RunRecord> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    files.sort();
    files
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|text| serde_json::from_str::<RunRecord>(&text).ok())
        .collect()
}

fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        "n/a".to_owned()
    } else {
        let change = i128::from(part) * 100 / i128::from(whole) - 100;
        format!("{change:+}%")
    }
}

/// The run as text, with the comparison with the previous run when there is one.
pub(crate) fn render(record: &RunRecord, previous: Option<&RunRecord>) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "suite {} (model {})", record.suite, record.model);
    for case in &record.cases {
        let _ = writeln!(
            text,
            "  {:<5} {:<34} {:>4} s  {:>2} tools  {:>7} in / {:>5} out",
            if case.passed { "pass" } else { "FAIL" },
            case.id,
            case.seconds,
            case.tools.len(),
            thousands(case.input_tokens),
            thousands(case.output_tokens)
        );
        for failure in &case.failures {
            let _ = writeln!(text, "          - {failure}");
        }
    }
    let total = record.cases.len();
    let passed = record.passed();
    let _ = writeln!(
        text,
        "{passed}/{total} passed, {} tokens in, {} out, {} s",
        thousands(record.input_tokens()),
        thousands(record.output_tokens()),
        record.seconds()
    );
    if let Some(before) = previous {
        let comparison = compare(before, record);
        let _ = writeln!(
            text,
            "against the previous run ({}, {}/{} passed): {} regression(s){}, {} fixed, input tokens {}",
            before.started_at,
            before.passed(),
            before.cases.len(),
            comparison.regressions.len(),
            if comparison.regressions.is_empty() {
                String::new()
            } else {
                format!(" ({})", comparison.regressions.join(", "))
            },
            comparison.fixed.len(),
            percent(record.input_tokens(), before.input_tokens())
        );
    } else {
        text.push_str("no earlier run of this suite to compare with\n");
    }
    text
}

/// `jarvis eval`.
pub async fn run_eval(client: &ApiClient, paths: &AppPaths, arguments: &[String]) -> ExitStatus {
    match arguments.get(1).map(String::as_str) {
        Some("run") => run_suite(client, paths, arguments).await,
        Some("history") => history(paths, arguments),
        _ => {
            eprintln!("{USAGE}");
            ExitStatus::Usage
        }
    }
}

struct Options {
    file: String,
    only: Vec<String>,
    json: bool,
}

fn parse_options(arguments: &[String]) -> Result<Options, String> {
    let mut file = None;
    let mut only = Vec::new();
    let mut json = false;
    let mut iterator = arguments.iter().skip(2);
    while let Some(argument) = iterator.next() {
        match argument.as_str() {
            "--json" => json = true,
            "--root" => {
                iterator.next();
            }
            "--case" => {
                only.push(iterator.next().ok_or("--case needs a case id")?.clone());
            }
            other if !other.starts_with("--") && file.is_none() => file = Some(other.to_owned()),
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    Ok(Options {
        file: file.ok_or("name the suite file")?,
        only,
        json,
    })
}

async fn run_suite(client: &ApiClient, paths: &AppPaths, arguments: &[String]) -> ExitStatus {
    let options = match parse_options(arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("jarvis: {message}\n{USAGE}");
            return ExitStatus::Usage;
        }
    };
    let text = match std::fs::read_to_string(&options.file) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("jarvis: {} could not be read: {error}", options.file);
            return ExitStatus::Usage;
        }
    };
    let suite = match parse_suite(&text) {
        Ok(suite) => suite,
        Err(message) => {
            eprintln!("jarvis: {message}");
            return ExitStatus::Usage;
        }
    };
    for wanted in &options.only {
        if !suite.cases.iter().any(|case| &case.id == wanted) {
            eprintln!("jarvis: the suite has no case {wanted:?}");
            return ExitStatus::Usage;
        }
    }
    if !options.json && !suite.description.is_empty() {
        eprintln!("{}: {}", suite.name, suite.description);
    }
    let model = jarvis_storage::settings::get(paths, "executor_model_name")
        .unwrap_or_else(|_| "unknown".to_owned());
    let started_at = UtcTimestamp::now(&SystemClock).to_string();
    let mut cases = Vec::new();
    for case in &suite.cases {
        if !options.only.is_empty() && !options.only.contains(&case.id) {
            continue;
        }
        if !options.json {
            eprintln!("  running {} ...", case.id);
        }
        let seen = observe(client, case).await;
        let checks = evaluate(&case.expect, &seen);
        cases.push(CaseRecord {
            id: case.id.clone(),
            passed: checks.iter().all(|c| c.passed),
            seconds: seen.seconds,
            tools: seen.tools.clone(),
            input_tokens: seen.input_tokens,
            output_tokens: seen.output_tokens,
            failures: checks
                .iter()
                .filter(|c| !c.passed)
                .map(|c| {
                    if c.detail.is_empty() {
                        c.name.clone()
                    } else {
                        format!("{}: {}", c.name, c.detail)
                    }
                })
                .collect(),
            answer_head: head(seen.answer.as_deref().unwrap_or(""), ANSWER_HEAD_CHARS),
        });
    }
    let record = RunRecord {
        suite: suite.name.clone(),
        model,
        started_at,
        cases,
    };
    // A partial run (--case) is shown but is not what the next full run is compared with.
    let full = options.only.is_empty();
    let previous = saved_runs(&history_dir(paths, &record.suite))
        .into_iter()
        .rfind(|run| run.cases.len() == suite.cases.len());
    if full && let Err(message) = save(paths, &record) {
        eprintln!("jarvis: {message}");
    }
    let all_passed = record.cases.iter().all(|case| case.passed);
    if options.json {
        let status = print_json(&record);
        if status != ExitStatus::Ok {
            return status;
        }
    } else {
        print!("{}", render(&record, previous.as_ref()));
    }
    if all_passed {
        ExitStatus::Ok
    } else {
        ExitStatus::RunFailed
    }
}

fn history(paths: &AppPaths, arguments: &[String]) -> ExitStatus {
    let name = arguments.get(2).filter(|a| !a.starts_with("--"));
    let root = paths.data().join("evals");
    let mut suites: Vec<String> = match name {
        Some(name) => vec![name.clone()],
        None => std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect(),
    };
    suites.sort();
    let mut any = false;
    for suite in suites {
        for run in saved_runs(&history_dir(paths, &suite)) {
            any = true;
            println!(
                "{}  {:<24} {}/{} passed  {} in / {} out  {}",
                run.started_at,
                run.suite,
                run.passed(),
                run.cases.len(),
                thousands(run.input_tokens()),
                thousands(run.output_tokens()),
                run.model
            );
        }
    }
    if !any {
        println!("no saved runs yet: `jarvis eval run SUITE.toml`");
    }
    ExitStatus::Ok
}

#[cfg(test)]
#[path = "eval_tests.rs"]
mod tests;
