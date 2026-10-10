//! `jarvis push test`: send one test message to the configured ntfy topic (`ADR-0155`).

use crate::api_client::ApiClient;
use crate::output::ExitStatus;
use crate::schedule::report;

const USAGE: &str = "usage: jarvis push test    # send one test message to daemon.push_topic (set it with `jarvis config set daemon.push_topic NAME`, a long unguessable name)";

/// Runs one `jarvis push` verb.
pub async fn run_push(client: &ApiClient, arguments: &[String]) -> ExitStatus {
    if arguments.get(1).map(String::as_str) != Some("test") {
        eprintln!("{USAGE}");
        return ExitStatus::Usage;
    }
    match client.push_test().await {
        Ok(()) => {
            println!("sent: check your phone for \"This is a test from JARVIS.\"");
            ExitStatus::Ok
        }
        Err(error) => report(&error),
    }
}
