//! Continuing a run that a restart cut off (`ADR-0155`).
//!
//! Recovery settles an interrupted run as failed, because that is the truth: the model stream that held its progress
//! is gone. A person who had asked for something and finds it silently dropped by an update or a crash has lost work
//! they do not know is lost, so after recovery a recent run gets **one continuation**: an ordinary new run in the same
//! conversation, which sees everything already said and done, told to check that before repeating any of it.
//!
//! It is an ordinary run, started the way every run is, so policy, approvals and audit apply unchanged. It is bounded
//! so a daemon that keeps dying cannot loop: a continuation is never continued, a run that is no longer the newest of
//! its conversation is left alone, nothing older than [`RECENT_NANOS`] is touched, and at most [`MAX_RESUMED`] start.

use jarvis_core::{SystemClock, UtcTimestamp};
use jarvis_protocol::StartRunRequest;

use crate::gateway::GatewayState;

/// How far back an interrupted run may start and still be continued: a task from yesterday is not wanted now.
const RECENT_NANOS: i128 = 2 * 3_600 * 1_000_000_000;

/// The most runs one restart continues.
const MAX_RESUMED: u32 = 3;

/// Begins a continuation's objective. Also what stops a continuation being continued again.
pub(crate) const RESUME_NOTICE: &str = "[resumed] A restart interrupted your previous attempt at this request. Before you do anything, \
look at what this conversation and your tools show was already done (files written, notes saved, messages sent) and do not repeat \
it, least of all anything that sends or changes something outside this computer. Then finish the request:\n\n";

/// The objective for a continuation of `original`, kept within the objective limit.
fn continuation_objective(original: &str) -> String {
    let room = jarvis_storage::MAX_OBJECTIVE_CHARS.saturating_sub(RESUME_NOTICE.chars().count());
    let kept: String = original.chars().take(room).collect();
    format!("{RESUME_NOTICE}{kept}")
}

/// Whether the owner allows continuations (on unless `daemon.resume_interrupted` is off).
fn enabled(state: &GatewayState) -> bool {
    state.settings().is_none_or(|context| {
        jarvis_storage::ConfigStore::from_paths(context.paths())
            .load()
            .is_ok_and(|loaded| loaded.config().daemon().resume_interrupted_enabled())
    })
}

/// Continues the runs in `settled` that qualify, returning how many were started.
pub(crate) async fn continue_interrupted(state: &GatewayState, settled: &[String]) -> u32 {
    if settled.is_empty() || !enabled(state) {
        return 0;
    }
    let now = UtcTimestamp::now(&SystemClock).unix_nanos();
    let Ok(since) = UtcTimestamp::from_unix_nanos(now - RECENT_NANOS) else {
        return 0;
    };
    let candidates = match jarvis_storage::continuable_interrupted_runs(
        state.database(),
        settled,
        since,
        &[RESUME_NOTICE, crate::delegate::DELEGATED_NOTICE],
        MAX_RESUMED,
    )
    .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(%error, "interrupted runs could not be checked for continuation");
            return 0;
        }
    };
    let mut started = 0;
    for run in candidates {
        let request = StartRunRequest {
            objective: continuation_objective(&run.objective),
            session_id: Some(run.session_id),
            idempotency_key: None,
            project_id: None,
        };
        if let Ok(reply) = state.start_and_drive(&request).await {
            tracing::info!(
                interrupted = run.run_id,
                continued_by = reply.run_id,
                "continued a run that a restart interrupted"
            );
            started += 1;
        } else {
            tracing::warn!(
                interrupted = run.run_id,
                "a run that a restart interrupted could not be continued"
            );
        }
    }
    started
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_continuation_names_the_original_and_stays_within_the_limit() {
        let short = continuation_objective("write the report");
        assert!(short.starts_with(RESUME_NOTICE) && short.ends_with("write the report"));
        let long = continuation_objective(&"x".repeat(10_000));
        assert!(long.chars().count() <= jarvis_storage::MAX_OBJECTIVE_CHARS);
    }
}
