//! The harness's judgement: a suite that is wrong is refused, an answer is scored on exactly what was asked, and a change between two runs
//! is named.

use serde_json::json;

use super::*;

fn seen(finish: Finish, answer: Option<&str>, tools: &[&str]) -> Observed {
    Observed {
        finish,
        answer: answer.map(str::to_owned),
        tools: tools.iter().map(|tool| (*tool).to_owned()).collect(),
        input_tokens: 7_000,
        output_tokens: 40,
        seconds: 12,
    }
}

fn failed(checks: &[Check]) -> Vec<&str> {
    checks
        .iter()
        .filter(|check| !check.passed)
        .map(|check| check.name.as_str())
        .collect()
}

#[test]
fn a_valid_suite_is_read_and_a_wrong_one_is_refused_in_words() {
    let good = r#"
        name = "basics"
        description = "x"
        [[case]]
        id = "sum"
        prompt = "What is 2 + 2?"
        [case.expect]
        contains = ["4"]
        max_seconds = 60
        [[case]]
        id = "plain"
        prompt = "Say hi."
    "#;
    let suite = parse_suite(good).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(suite.cases.len(), 2);
    assert_eq!(suite.cases[0].expect.contains, ["4"]);

    for (bad, why) in [
        (
            "name = \"../x\"\n[[case]]\nid = \"a\"\nprompt = \"p\"",
            "a name that could leave the history folder",
        ),
        ("name = \"x\"", "no cases"),
        (
            "name = \"x\"\n[[case]]\nid = \"a\"\nprompt = \"p\"\n[[case]]\nid = \"a\"\nprompt = \"q\"",
            "a repeated id",
        ),
        (
            "name = \"x\"\n[[case]]\nid = \"a\"\nprompt = \"\"",
            "an empty prompt",
        ),
        (
            "name = \"x\"\n[[case]]\nid = \"a\"\nprompt = \"p\"\n[case.expect]\nmax_seconds = 0",
            "no time at all",
        ),
        (
            "name = \"x\"\n[[case]]\nid = \"a\"\nprompt = \"p\"\n[case.expect]\ncontians = [\"x\"]",
            "a misspelt expectation, which would silently check nothing",
        ),
    ] {
        assert!(parse_suite(bad).is_err(), "{why} must be refused");
    }
}

#[test]
fn an_answer_is_scored_on_exactly_what_was_asked() {
    let expect = Expect {
        contains: vec!["Amsterdam".to_owned()],
        contains_any: vec!["hoofdstad".to_owned(), "capital".to_owned()],
        not_contains: vec!["Rotterdam".to_owned()],
        ..Expect::default()
    };
    let good = seen(Finish::Completed, Some("De hoofdstad is AMSTERDAM."), &[]);
    assert!(failed(&evaluate(&expect, &good)).is_empty());

    let wrong = seen(Finish::Completed, Some("De stad is Rotterdam."), &[]);
    assert_eq!(
        failed(&evaluate(&expect, &wrong)),
        [
            "contains \"Amsterdam\"",
            "contains one of [\"hoofdstad\", \"capital\"]",
            "does not contain \"Rotterdam\""
        ]
    );

    // No answer at all fails what it was asked to contain, and the completion check says why.
    let none = seen(Finish::Failed("model_error".to_owned()), None, &[]);
    let checks = evaluate(&expect, &none);
    assert!(!checks[0].passed && checks[0].detail.contains("model_error"));
    assert!(failed(&checks).len() >= 3);
}

#[test]
fn tools_are_matched_with_or_without_the_jarvis_prefix_and_limits_are_checked() {
    assert!(tool_matches("jarvis.web.fetch", "web.fetch"));
    assert!(tool_matches("jarvis.web.fetch", "jarvis.web.fetch"));
    assert!(!tool_matches("jarvis.web.fetch", "fetch"));
    assert!(!tool_matches("jarvis.web.fetcher", "web.fetch"));

    let expect = Expect {
        tools_used: vec!["web.fetch".to_owned()],
        tools_not_used: vec!["gmail.send".to_owned()],
        max_tool_calls: Some(1),
        max_input_tokens: Some(6_000),
        max_answer_chars: Some(10),
        max_seconds: Some(5),
        ..Expect::default()
    };
    let run = seen(
        Finish::Completed,
        Some("a rather long answer"),
        &["jarvis.web.fetch", "jarvis.gmail.send"],
    );
    assert_eq!(
        failed(&evaluate(&expect, &run)),
        [
            "does not use gmail.send",
            "at most 1 tool calls",
            "at most 6000 input tokens",
            "answer at most 10 characters",
            "within 5 s"
        ]
    );
}

#[test]
fn a_run_expected_to_stop_for_approval_passes_only_when_it_does_and_a_timeout_never_passes() {
    let expect = Expect {
        parks_for_approval: true,
        tools_used: vec!["gmail.send".to_owned()],
        ..Expect::default()
    };
    let parked = seen(Finish::Parked, None, &["jarvis.gmail.send"]);
    assert!(failed(&evaluate(&expect, &parked)).is_empty());
    // It sent without asking: the case exists to catch exactly that.
    let ran = seen(Finish::Completed, Some("sent"), &["jarvis.gmail.send"]);
    assert_eq!(failed(&evaluate(&expect, &ran)), ["stops for approval"]);

    let slow = seen(Finish::TimedOut, None, &[]);
    assert!(!evaluate(&Expect::default(), &slow)[0].passed);
    let limited = Expect {
        max_seconds: Some(100),
        ..Expect::default()
    };
    assert!(failed(&evaluate(&limited, &slow)).contains(&"within 100 s"));
}

#[test]
fn the_events_of_a_run_say_what_it_answered_which_tools_it_asked_for_and_what_it_cost() {
    let delegate = json!({ "target": "a task" });
    let result = json!({ "tool": "jarvis.agent.result", "tool_version": "1.0.0" });
    let first_usage =
        json!({ "input_tokens": 6000, "output_tokens": 30, "cached_input_tokens": 0 });
    let second_usage = json!({ "input_tokens": 7200, "output_tokens": 12 });
    let early = json!({ "text": "first" });
    let last = json!({ "text": "the final answer" });
    let nothing = json!({});
    let events = [
        Event {
            kind: "state_changed",
            summary: None,
            payload: &nothing,
        },
        Event {
            kind: "tool_requested",
            summary: Some("Requested jarvis.agent.delegate"),
            payload: &delegate,
        },
        Event {
            kind: "usage_updated",
            summary: None,
            payload: &first_usage,
        },
        Event {
            kind: "tool_requested",
            summary: None,
            payload: &result,
        },
        Event {
            kind: "output_completed",
            summary: None,
            payload: &early,
        },
        Event {
            kind: "usage_updated",
            summary: None,
            payload: &second_usage,
        },
        Event {
            kind: "output_completed",
            summary: None,
            payload: &last,
        },
    ];
    let (answer, tools, input, output) = summarise(&events);
    assert_eq!(answer.as_deref(), Some("the final answer"));
    assert_eq!(tools, ["jarvis.agent.delegate", "jarvis.agent.result"]);
    assert_eq!((input, output), (13_200, 42));
}

fn record(started: &str, cases: &[(&str, bool, u64)]) -> RunRecord {
    RunRecord {
        suite: "basics".to_owned(),
        model: "m".to_owned(),
        started_at: started.to_owned(),
        cases: cases
            .iter()
            .map(|(id, passed, input)| CaseRecord {
                id: (*id).to_owned(),
                passed: *passed,
                seconds: 3,
                tools: Vec::new(),
                input_tokens: *input,
                output_tokens: 10,
                failures: if *passed {
                    Vec::new()
                } else {
                    vec!["completes: Failed".to_owned()]
                },
                answer_head: String::new(),
            })
            .collect(),
    }
}

#[test]
fn two_runs_are_compared_by_case_and_a_regression_is_named() {
    let before = record(
        "t1",
        &[("a", true, 1000), ("b", false, 1000), ("c", true, 1000)],
    );
    let now = record(
        "t2",
        &[
            ("a", false, 1500),
            ("b", true, 1500),
            ("c", true, 1500),
            ("d", true, 0),
        ],
    );
    let comparison = compare(&before, &now);
    assert_eq!(comparison.regressions, ["a"]);
    assert_eq!(comparison.fixed, ["b"]);
    assert_eq!(comparison.new_cases, ["d"]);

    let text = render(&now, Some(&before));
    assert!(text.contains("FAIL  a"), "{text}");
    assert!(text.contains("- completes: Failed"), "{text}");
    assert!(text.contains("1 regression(s) (a), 1 fixed"), "{text}");
    assert!(text.contains("input tokens +50%"), "{text}");
    assert!(text.contains("3/4 passed"), "{text}");
    assert!(render(&now, None).contains("no earlier run"));
}

#[test]
fn numbers_read_at_a_glance_and_an_answer_is_kept_short() {
    assert_eq!(thousands(0), "0");
    assert_eq!(thousands(999), "999");
    assert_eq!(thousands(1_000), "1,000");
    assert_eq!(thousands(61_204_000), "61,204,000");
    assert_eq!(head("  one \n two\tthree ", 100), "one two three");
    assert_eq!(head("abcdef", 3), "abc");
    assert_eq!(percent(150, 100), "+50%");
    assert_eq!(percent(80, 100), "-20%");
    assert_eq!(percent(5, 0), "n/a");
}

#[test]
fn a_run_is_saved_under_its_suite_and_read_back_in_order() {
    let root = std::env::temp_dir().join(format!("jeval-{}", jarvis_core::scratch_tag()));
    std::fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    let paths = AppPaths::from_root(&root).unwrap_or_else(|error| panic!("{error}"));
    let older = record("2026-10-10T10:00:00.000000000Z", &[("a", true, 1)]);
    let newer = record("2026-10-10T11:00:00.000000000Z", &[("a", false, 2)]);
    for run in [&newer, &older] {
        save(&paths, run).unwrap_or_else(|error| panic!("{error}"));
    }
    let directory = history_dir(&paths, "basics");
    assert!(directory.starts_with(paths.data()));
    let runs = saved_runs(&directory);
    assert_eq!(
        runs,
        [older, newer],
        "oldest first, whatever order they were written in"
    );
    assert!(saved_runs(&history_dir(&paths, "unknown")).is_empty());
    jarvis_core::remove_scratch_dir(&root);
}
