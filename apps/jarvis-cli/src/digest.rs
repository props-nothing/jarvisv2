//! `jarvis digest`: what JARVIS did while you were away (`ADR-0155`).

use jarvis_protocol::DigestReply;

use crate::api_client::ApiClient;
use crate::output::ExitStatus;
use crate::schedule::{print_json, report};

const USAGE: &str = "usage: jarvis digest [HOURS] [--json]    # runs, outcomes, what each project decided, failures and tokens; 24 hours by default (up to 744)";

/// Runs `jarvis digest`.
pub async fn run_digest(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    let mut hours = 24_u32;
    let mut json = false;
    let mut skip = false;
    for argument in arguments.iter().skip(1) {
        if skip {
            skip = false;
            continue;
        }
        match argument.as_str() {
            "--json" => json = true,
            "--root" => skip = true,
            other => {
                let Ok(value) = other.parse::<u32>() else {
                    eprintln!("jarvis: {other:?} is not a number of hours");
                    eprintln!("{USAGE}");
                    return ExitStatus::Usage;
                };
                hours = value;
            }
        }
    }
    match client.digest(hours).await {
        Ok(reply) if json => print_json(&reply),
        Ok(reply) => {
            print!("{}", render(&reply));
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}

/// A token count a person can read at a glance.
fn tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{}.{}M", count / 1_000_000, count % 1_000_000 / 100_000)
    } else if count >= 1_000 {
        format!("{}.{}k", count / 1_000, count % 1_000 / 100)
    } else {
        count.to_string()
    }
}

/// The digest as text.
pub(crate) fn render(digest: &DigestReply) -> String {
    use std::fmt::Write as _;
    let mut text = String::new();
    let window = if digest.hours.is_multiple_of(24) {
        let days = digest.hours / 24;
        format!("{days} day{}", if days == 1 { "" } else { "s" })
    } else {
        format!("{} hours", digest.hours)
    };
    let _ = writeln!(text, "Last {window}: {} run(s)", digest.runs);
    if digest.runs == 0 {
        text.push_str("  nothing ran\n");
        return text;
    }
    let _ = writeln!(
        text,
        "  {} finished, {} failed, {} cancelled, {} waiting for you",
        digest.succeeded, digest.failed, digest.cancelled, digest.waiting
    );
    if digest.input_tokens + digest.output_tokens == 0 {
        // Not every provider reports usage; zero here means unknown, not free.
        text.push_str("  tokens: not reported by the model provider\n");
    } else {
        let _ = writeln!(
            text,
            "  tokens: {} in, {} out",
            tokens(digest.input_tokens),
            tokens(digest.output_tokens)
        );
    }
    for project in &digest.projects {
        let _ = writeln!(text, "\n{}  ({} run(s))", project.name, project.runs);
        for note in &project.highlights {
            let _ = writeln!(text, "  [{}] {}", note.kind, note.text.replace('\n', " "));
        }
    }
    if !digest.problems.is_empty() {
        text.push_str("\nFailed:\n");
        for problem in &digest.problems {
            let objective: String = problem.objective.chars().take(90).collect();
            let _ = writeln!(text, "  {objective}  ({})", problem.error_code);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use jarvis_protocol::{DigestHighlight, DigestProblem, DigestProject};

    use super::*;

    fn digest() -> DigestReply {
        DigestReply {
            hours: 24,
            runs: 5,
            succeeded: 3,
            failed: 1,
            cancelled: 0,
            waiting: 1,
            input_tokens: 123_456,
            output_tokens: 2_500_000,
            projects: vec![DigestProject {
                name: "Prospecting".to_owned(),
                runs: 4,
                highlights: vec![DigestHighlight {
                    kind: "decision".to_owned(),
                    text: "Dentists first".to_owned(),
                    created_at: "2026-01-01T00:00:00Z".to_owned(),
                }],
            }],
            problems: vec![DigestProblem {
                objective: "[scheduled] check mail".to_owned(),
                error_code: "interrupted_by_restart".to_owned(),
            }],
        }
    }

    #[test]
    fn the_text_leads_with_the_outcome_and_names_what_needs_the_owner() {
        let text = render(&digest());
        assert!(text.starts_with("Last 1 day: 5 run(s)\n"), "{text}");
        assert!(text.contains("1 waiting for you") && text.contains("123.4k in, 2.5M out"));
        assert!(
            text.contains("Prospecting  (4 run(s))") && text.contains("[decision] Dentists first")
        );
        assert!(text.contains("[scheduled] check mail  (interrupted_by_restart)"));
    }

    #[test]
    fn unreported_tokens_are_not_shown_as_zero() {
        let unknown = DigestReply {
            input_tokens: 0,
            output_tokens: 0,
            ..digest()
        };
        assert!(render(&unknown).contains("not reported by the model provider"));
    }

    #[test]
    fn a_quiet_window_says_so() {
        let quiet = DigestReply {
            runs: 0,
            projects: Vec::new(),
            problems: Vec::new(),
            ..digest()
        };
        assert!(render(&quiet).contains("nothing ran"));
    }
}
