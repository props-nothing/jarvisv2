//! `jarvis ask`: start a run through the daemon's API and render its stream.
//!
//! # Scope: there is no `chat` here on purpose
//!
//! `TODO.md` `P2-008` names "`ask` and `chat`". `ask` is implemented; `chat` is deliberately not,
//! and the reason is a real gap rather than an omission. A chat loop needs a session that persists
//! across turns and a model that answers, and neither exists yet: nothing in `jarvisd` invokes a
//! model, so a run reaches `received` and stays there. A `chat` built now could only print a
//! `received` run per turn, which would look like a conversation while recording none. The session
//! read model and the model adapter are `P2-009`, and `chat` belongs with them.
//!
//! # The CLI contains no orchestration
//!
//! `P2-008` requires it: "the CLI must contain no orchestration logic". Everything this module does
//! is transport work — start a run through the daemon's API, read the daemon's stream, and render
//! what the daemon recorded. It makes no policy decision, assembles no context, and chooses no model.
//!
//! # Identity comes from the daemon, never from the client
//!
//! The workspace and user are resolved by the daemon from its seeded local identity. The client
//! sends an objective and nothing else. A client-supplied workspace identifier would be a claim
//! rather than proof of access, which `docs/architecture/identity-and-workspaces.md` forbids.
//!
//! # Rendering distinguishes intent, progress, output, and settlement
//!
//! `docs/development/definition-of-done.md` requires user-visible wording to keep those apart. So
//! the run identifier and the accepted objective are shown as the *request*, each state change is
//! an operational line on stderr, answer text goes to stdout as it arrives, and the terminal event
//! is reported with the outcome the daemon recorded — including "cancelled" and "failed", which are
//! not successes and do not exit `0`.
//!
//! Answer text goes to stdout while progress goes to stderr so `jarvis ask ... > answer.txt`
//! captures the answer rather than a transcript of the run.

use std::io::Write;

use jarvis_protocol::{RunReply, RunStreamFrame, StreamReading, output_text, state_name};

use crate::api_client::{ApiClient, ApiError};
use crate::output::ExitStatus;

/// Consecutive keep-alive frames with no event between them before a run is reported as stalled.
///
/// The daemon sends a keep-alive every 15 seconds for as long as a run is active, so this is about
/// two minutes of an active run producing nothing. It bounds a stall that `read_timeout` cannot see,
/// because the keep-alives themselves keep resetting that timeout.
pub(crate) const MAX_IDLE_KEEP_ALIVES: u32 = 8;

/// Streams one run to completion, returning the process exit status.
pub(crate) async fn drive(client: &ApiClient, objective: &str) -> ExitStatus {
    // The endpoint is named before the request so a transport failure is diagnosable: "the daemon
    // API could not be reached" is not actionable without knowing which address was tried, and the
    // HTTP transport is separately enabled so the address is configuration rather than a constant.
    eprintln!("jarvis: asking {}", client.host());

    let reply = match client.start_run(objective).await {
        Ok(reply) => reply,
        Err(error) => return report_error(&error),
    };

    // The identifiers are printed as the *request* that was accepted: they are what an operator or
    // a later `jarvis status` call needs to correlate this run with the daemon's record.
    println!(
        "run {} session {} accepted state={} version={}",
        reply.run_id, reply.session_id, reply.state, reply.version
    );

    let mut stream = match client.open_stream(&reply.run_id).await {
        Ok(stream) => stream,
        Err(error) => return report_error(&error),
    };

    // A run's stream begins with the state it was already in, so the first `state_changed` event is
    // the objective's acceptance, not progress. Rendering it would print the same state twice.
    let mut seen_first_state = false;
    // Consecutive keep-alive frames seen with no event between them. The daemon sends one every 15
    // seconds for as long as a run is active, and `read_timeout` cannot notice a stalled run because
    // the keep-alives keep resetting it. The stream reports events in a steady flow when a run is
    // progressing, so a long stretch of nothing but keep-alives is a stall, and without a bound the
    // command would wait forever with no explanation.
    let mut idle_keep_alives = 0_u32;

    loop {
        let frame = match stream.next_frame().await {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                // The daemon closes the stream when the run settles, and every settled run has a
                // terminal event. Reaching here means the stream ended without one, so the run's
                // outcome is unknown rather than successful and saying so is the honest report.
                eprintln!(
                    "jarvis: the run stream ended without a terminal event; the run's outcome is unknown"
                );
                return ExitStatus::Unavailable;
            }
            Err(error) => return report_error(&error),
        };

        let event = match frame {
            RunStreamFrame::Event(event) => event,
            // A mid-stream failure carries the daemon's error envelope, so it is mapped onto the
            // same exit statuses as a refused request rather than reported as a transport fault.
            RunStreamFrame::Error(error) => {
                eprintln!("jarvis: the run stream reported an error: {error}");
                return ExitStatus::from_code(error.code);
            }
            // The daemon sends keep-alives as SSE comments so they never enter the durable log.
            // Printing one would present transport noise as run activity, so they are counted and
            // not rendered.
            RunStreamFrame::KeepAlive => {
                idle_keep_alives += 1;
                if idle_keep_alives >= MAX_IDLE_KEEP_ALIVES {
                    // Reported rather than waited on: a run that produces no event across this span
                    // is stalled, and the most likely cause is that nothing is executing runs yet.
                    // Without this bound the command would wait forever, because keep-alives defeat
                    // the read timeout that would otherwise notice.
                    eprintln!(
                        "jarvis: run {} has produced no event across {} keep-alives; it is stalled, not working",
                        reply.run_id, idle_keep_alives
                    );
                    eprintln!(
                        "jarvis: nothing invokes a model until P2-009, so an accepted run stays in `received`"
                    );
                    return ExitStatus::Unavailable;
                }
                continue;
            }
        };
        idle_keep_alives = 0;

        match StreamReading::of(event.kind) {
            StreamReading::Output => {
                if let Some(text) = output_text(&event.payload) {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
            }
            StreamReading::StateChanged => {
                if seen_first_state {
                    if let Some(state) = state_name(&event.payload) {
                        eprintln!("jarvis: state {state}");
                    }
                } else {
                    seen_first_state = true;
                }
            }
            StreamReading::Activity => {
                if let Some(summary) = &event.summary {
                    eprintln!("jarvis: {summary}");
                }
            }
            reading @ (StreamReading::Completed
            | StreamReading::Failed
            | StreamReading::Cancelled) => {
                // The stream closes after a terminal event, so the loop ends here. The answer is
                // flushed first so a final line is not left in a block buffer on exit.
                let _ = std::io::stdout().flush();
                let settled = match client.read_run(&reply.run_id).await {
                    Ok(settled) => settled,
                    Err(error) => return report_error(&error),
                };
                return report_settlement(&settled, reading);
            }
        }
    }
}

/// Reports how a run settled, and maps it onto an exit status.
///
/// Reads the run's settled record rather than trusting the event alone: the event says what
/// happened, and the record is what `jarvis` could describe later. Reporting from the event would
/// let a display detail disagree with the durable row.
fn report_settlement(settled: &RunReply, reading: StreamReading) -> ExitStatus {
    let outcome = settled
        .outcome
        .map_or("unknown", jarvis_core::RunOutcome::as_str);
    let error_code = settled.error_code.as_deref().unwrap_or("none");
    eprintln!(
        "jarvis: run {} settled state={} outcome={outcome} error={error_code}",
        settled.run_id, settled.state
    );

    match reading {
        StreamReading::Completed => ExitStatus::Ok,
        StreamReading::Cancelled => {
            eprintln!("jarvis: the run was cancelled");
            ExitStatus::Cancelled
        }
        StreamReading::Failed => {
            // The failure code is the daemon's bounded `error_code`, which is safe to print and is
            // the actionable part of a failure report.
            if let Some(code) = &settled.error_code {
                eprintln!("jarvis: the run failed with code {code}");
            } else {
                eprintln!("jarvis: the run failed");
            }
            ExitStatus::RunFailed
        }
        other => {
            // Unreachable by construction: the caller only reaches here for a terminal reading, and
            // `StreamReading::is_terminal` names exactly those three. Reported rather than panicking,
            // because a client that panics on an unexpected event is worse than one that says it did
            // not understand what arrived.
            let _ = other;
            eprintln!("jarvis: the run settled with an unclassified event; reporting unknown");
            ExitStatus::Internal
        }
    }
}

/// Reports a request failure and maps it onto an exit status.
fn report_error(error: &ApiError) -> ExitStatus {
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
