//! `jarvis ask` and `jarvis chat`: drive a run through the daemon's API and render its stream.
//!
//! # What `chat` adds, and why it is a client change rather than a new mechanism
//!
//! A multi-turn conversation is a sequence of **runs sharing one session**. The second turn is a new
//! run, not a mutation of the first, and the daemon replays the session's transcript into the model
//! call. That is the whole of the difference: `chat` remembers a session identifier between turns and
//! sends it with each objective, so nothing about orchestration moves into the client.
//!
//! The session identifier is one the **daemon** issued, printed on the first turn so an operator can
//! find the conversation later. The client never invents one: a session identifier is guessable, and
//! a client that chose its own could name a conversation it was never granted. The daemon verifies
//! every identifier against the profile's workspace and user before writing a run into it.
//!
//! # The CLI contains no orchestration
//!
//! `P2-008` requires it: "the CLI must contain no orchestration logic". Everything this module does
//! is transport work — start a run through the daemon's API, read the daemon's stream, and render
//! what the daemon recorded. It makes no policy decision, assembles no context, chooses no model, and
//! decides nothing about what history the model sees; that selection is the daemon's context
//! assembler and its result is the manifest the daemon records.
//!
//! # Identity comes from the daemon, never from the client
//!
//! The workspace and user are resolved by the daemon from its seeded local identity. The client
//! sends an objective and, for a continuation, a session identifier it was given.
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

use std::io::{BufRead, Write};

use jarvis_core::ErrorCode;
use jarvis_protocol::{RunReply, RunStreamFrame, StreamReading, output_text, state_name};

use crate::api_client::{ApiClient, ApiError};
use crate::output::ExitStatus;

/// Consecutive keep-alive frames with no event between them before a run is reported as stalled.
///
/// The daemon sends a keep-alive every 15 seconds for as long as a run is active, so this is about
/// two minutes of an active run producing nothing. It bounds a stall that `read_timeout` cannot see,
/// because the keep-alives themselves keep resetting that timeout.
pub(crate) const MAX_IDLE_KEEP_ALIVES: u32 = 8;

/// Characters allowed in one turn's objective.
///
/// Matches the storage schema's bound, so a turn the daemon would refuse is refused here with an
/// explanation instead of becoming a failed request. Counting characters rather than bytes follows
/// the same rule the schema uses: it bounds a `TEXT` column with `length()`, which counts characters.
const MAX_TURN_CHARS: usize = 4_096;

/// Commands a chat session understands, typed on their own line.
const CHAT_EXIT: [&str; 2] = [":quit", ":exit"];

/// Runs one objective through the daemon's HTTP API and renders its stream.
///
/// # Why this needs the daemon's configuration
///
/// The HTTP transport is separately enabled and its port is configuration, so the CLI reads the
/// same profile configuration the daemon does and targets whatever the daemon was told to bind. A
/// hard-coded port would work on a default install and silently target nothing on a configured one.
pub(crate) async fn drive(client: &ApiClient, objective: &str) -> ExitStatus {
    start_and_render(client, objective, None).await.status
}

/// Runs an interactive conversation, one turn per line of standard input.
///
/// # Why the session is printed before the first answer
///
/// A conversation that cannot be found again is not durable, so the session identifier is reported
/// as part of the *accepted request* on the first turn. `P4-008` adds the inspect and export surface
/// that a user would use it with; until then this line is the only handle on a stored conversation.
///
/// # Why a failed turn does not end the conversation
///
/// A run can fail for reasons that say nothing about the conversation: a provider outage, a refusal,
/// a cancelled request. Ending the loop would discard the session, so the failure is reported and the
/// next line is read. A failure of the **session** is different — a closed or foreign session cannot
/// accept another turn — and that ends the loop, because every later turn would fail the same way.
pub(crate) async fn converse(client: &ApiClient) -> ExitStatus {
    eprintln!(
        "jarvis: chatting with {}. End a turn with a blank line, or type :quit.",
        client.host()
    );
    eprintln!(
        "jarvis: the daemon stores this conversation; the session identifier is printed below."
    );

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let mut session_id: Option<String> = None;
    let mut turns = 0_u32;
    // The last turn's failure, carried out of the loop so a script can see that something in the
    // conversation went wrong even though the conversation itself continued.
    let mut last_failure = ExitStatus::Ok;

    loop {
        eprint!("jarvis> ");
        let _ = std::io::stderr().flush();

        let Some(Ok(line)) = lines.next() else {
            // Input ended, or could not be read. Neither is an error worth a stack of diagnostics, and
            // ending quietly is what a user expects from a closed pipe or a typed control-D.
            break;
        };
        let turn = line.trim();
        if turn.is_empty() {
            continue;
        }
        if CHAT_EXIT.contains(&turn) {
            break;
        }
        if turn.chars().count() > MAX_TURN_CHARS {
            // Refused here rather than sent, so the message names the actual problem. The daemon
            // would refuse it too, but as a validation failure that does not say which bound was
            // exceeded or by how much.
            eprintln!(
                "jarvis: that turn is {} characters, over the {MAX_TURN_CHARS} allowed; shorten it",
                turn.chars().count()
            );
            continue;
        }

        let outcome = start_and_render(client, turn, session_id.as_deref()).await;
        // The session is remembered from the daemon's reply, not from a value the client chose, so a
        // daemon that started a different session than requested cannot leave the client addressing
        // one that does not exist.
        if let Some(accepted) = outcome.session_id {
            session_id = Some(accepted);
        }
        turns += 1;

        if outcome.session_rejected {
            eprintln!(
                "jarvis: the daemon refused the session, so the conversation cannot continue"
            );
            return outcome.status;
        }
        // A terminal *run* failure is reported and the loop continues; only the exit status of the
        // last turn is carried out, so a script can still see that something went wrong.
        if outcome.status != ExitStatus::Ok {
            last_failure = outcome.status;
        }
    }

    eprintln!(
        "jarvis: {turns} turn(s) recorded in session {}",
        session_id.as_deref().unwrap_or("(none)")
    );
    last_failure
}

/// The result of rendering one run.
struct TurnOutcome {
    /// The session the daemon recorded the run in.
    session_id: Option<String>,
    /// Whether the daemon refused the session itself, rather than the run failing.
    session_rejected: bool,
    /// The exit status for this turn.
    status: ExitStatus,
}

/// Starts a run in a session and renders its stream.
async fn start_and_render(
    client: &ApiClient,
    objective: &str,
    session_id: Option<&str>,
) -> TurnOutcome {
    // The endpoint is named before the request so a transport failure is diagnosable: "the daemon
    // API could not be reached" is not actionable without knowing which address was tried, and the
    // HTTP transport is separately enabled so the address is configuration rather than a constant.
    eprintln!("jarvis: asking {}", client.host());

    let reply = match client.start_run_in_session(objective, session_id).await {
        Ok(reply) => reply,
        Err(error) => {
            return TurnOutcome {
                session_id: None,
                // A session the daemon refused is distinguishable by its status: the request was
                // well-formed and the session it named was not usable. Any other refusal is about
                // the request itself and does not end a conversation.
                session_rejected: matches!(&error, ApiError::Refused(wire) if wire.code == ErrorCode::Conflict),
                status: report_error(&error),
            };
        }
    };

    // The identifiers are printed as the *request* that was accepted: they are what an operator or
    // a later `jarvis status` call needs to correlate this run with the daemon's record.
    println!(
        "run {} session {} accepted state={} version={}",
        reply.run_id, reply.session_id, reply.state, reply.version
    );

    let mut stream = match client.open_stream(&reply.run_id).await {
        Ok(stream) => stream,
        Err(error) => {
            return TurnOutcome {
                session_id: Some(reply.session_id.clone()),
                session_rejected: false,
                status: report_error(&error),
            };
        }
    };

    TurnOutcome {
        session_id: Some(reply.session_id.clone()),
        session_rejected: false,
        status: consume_turn(client, &reply, &mut stream).await,
    }
}

/// Renders a run's stream until it settles, and reports the exit status for the turn.
///
/// Separated from the start so neither function exceeds the length lint, and because the two do
/// different things: starting is a request that can be refused, and rendering is a loop over what the
/// daemon recorded.
async fn consume_turn(
    client: &ApiClient,
    reply: &RunReply,
    stream: &mut crate::api_client::RunEventStream,
) -> ExitStatus {
    // A run's stream begins with the state it was already in, so the first `state_changed` event is
    // the objective's acceptance, not progress. Rendering it would print the same state twice.
    let mut seen_first_state = false;
    // Consecutive keep-alive frames seen with no event between them. The daemon sends one every 15
    // seconds for as long as a run is active, and `read_timeout` cannot notice a stalled run because
    // the keep-alives keep resetting it.
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
                    // Reported rather than waited on. An accepted run that is never executed is the
                    // failure this bound exists for: the daemon can be configured without an
                    // executor, in which case runs are recorded and never driven, and without this
                    // the command would wait forever on keep-alives that defeat the read timeout.
                    eprintln!(
                        "jarvis: run {} has produced no event across {} keep-alives; it is stalled, not working",
                        reply.run_id, idle_keep_alives
                    );
                    eprintln!(
                        "jarvis: check that daemon.executor_model is set, or the run is recorded and never executed"
                    );
                    return ExitStatus::Unavailable;
                }
                continue;
            }
        };
        idle_keep_alives = 0;

        match StreamReading::of(event.kind) {
            StreamReading::OutputDelta => {
                if let Some(text) = output_text(&event.payload) {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
            }
            // The completed event repeats the WHOLE answer, so nothing is printed for it. Printing
            // its text would duplicate every fragment this loop already rendered, which is exactly
            // what happened before the two kinds were told apart. It is still a useful signal, so
            // its character count goes to stderr where progress belongs.
            StreamReading::OutputCompleted => {
                if let Some(chars) = event
                    .payload
                    .get("chars")
                    .and_then(serde_json::Value::as_u64)
                {
                    eprintln!("jarvis: answer complete ({chars} characters)");
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
