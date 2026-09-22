//! The native run executor: drive one run from `received` to a terminal state.
//!
//! This is what turns the run API from a **transport** into an executor. Before it, a run reached
//! `received` and stayed there; now a request is assembled into a model call, recorded as run events,
//! and settled.
//!
//! # Why this lives in `jarvisd`
//!
//! `docs/architecture/repository-layout.md` allows `jarvis-application` to depend only on
//! `jarvis-core`, and its dependency diagram has **no arrow from application into an adapter** —
//! providers are reached by binaries, which compose. `jarvis-models` is an adapter crate, so the
//! composition happens here, exactly where ADR-0011 already places the routing that consumes it.
//! Adding `jarvis-models` to `jarvis-application` would have inverted that diagram to save one
//! `use` statement.
//!
//! # The order of writes is forced, not chosen
//!
//! `jarvis_storage::settle_run` exists because appending a terminal event after a terminal
//! transition is refused, and appending before it records a settlement that did not happen. So the
//! non-terminal events are appended while the run is open, and the settlement and its event are
//! written together at the end.
//!
//! # Cancellation is a request that this loop observes
//!
//! `A04` requires that cancellation stops new work and settles once. The loop re-reads the run
//! between phases, so a cancellation requested while the model call was in flight is seen before
//! any further work starts, and the run settles `cancelled` from the observed state rather than
//! from a cached one. The model call itself is dropped when its future is dropped, which is the
//! adapter's documented cancellation behavior.
//!
//! # Every failure is classified, and a truncated answer is not a success
//!
//! A provider failure settles the run `failed` with the domain error code the model error maps to.
//! A stream that ends without a terminal event is `AmbiguousEffect`, never `completed`, because a
//! truncated answer is indistinguishable from a complete one except by the finish reason.

use std::sync::Arc;

use jarvis_core::{
    ContextBudget, ContextItem, ContextPriority, ContextSource, ContextSourceKind, ContextTrust,
    CorrelationId, EventSummary, InclusionReason, NewMessage, RunErrorCode, RunEventKind,
    RunEventPayload, RunState, RunTransition, Sensitivity, SystemClock, UtcTimestamp,
    assemble_context,
};
use jarvis_models::{
    ChatMessage, ChatRequest, FinishReason, ModelGateway, ModelId, Placement, StreamEvent,
    StreamValidator,
};
use jarvis_storage::{
    DatabaseError, NewRunEvent, SqliteDatabase, StoredRun, TerminalTransition, append_run_event,
    find_run, settle_run,
};

/// Characters of the objective carried in the first context item's source reference.
///
/// The reference is bounded by the context contract, so a long objective is truncated for the
/// *reference* only. The objective itself is never truncated: it is what the model is asked to do.
const MAX_OBJECTIVE_REFERENCE_CHARS: usize = 120;

/// Token budget for a run's context.
///
/// `P2-009` sends policy text and the objective and nothing else, because memory retrieval is
/// `P4-004`. The budget is still explicit rather than unlimited so the reserved tiers are exercised
/// and a later retrieval step draws on a measured remainder instead of on everything.
const TOTAL_CONTEXT_TOKENS: u32 = 8_192;

/// Tokens reserved for immutable policy text.
const RESERVED_POLICY_TOKENS: u32 = 512;

/// Tokens reserved for the current user intent.
const RESERVED_USER_INTENT_TOKENS: u32 = 1_024;

/// Maximum tokens any single source kind may contribute.
const PER_SOURCE_CAP_TOKENS: u32 = 4_096;

/// Model calls allowed for one run before it is failed rather than looped.
///
/// One, because `P2-009` does not plan or use tools: a second call would mean the loop is retrying
/// blindly, and a bounded retry policy belongs with the step planner rather than here.
const MAX_MODEL_CALLS: u32 = 1;

/// Events read back when locating a run's completed answer.
///
/// Well above any answer this slice produces, and bounded rather than unbounded so the read cannot
/// grow with a runaway run. A run that exceeded it would be one this executor did not produce.
const MAX_EVENTS_READ: u32 = 1_000;

/// The system prompt for a native run.
///
/// Authoritative text, and the only instruction-bearing content in the request. Kept short and
/// factual: this slice produces an answer, and a larger prompt would imply a tool or memory surface
/// that does not exist yet.
const SYSTEM_POLICY: &str = "You are JARVIS, a local assistant. Answer the user's request directly and concisely. \
Do not claim to have performed actions you did not perform.";

/// Maximum characters of an error message copied into an event payload.
///
/// The event payload is bounded in bytes by the storage contract, and a provider message can be
/// long. Truncating keeps the payload storable while preserving the beginning, which is where the
/// actionable part usually is.
const MAX_EVENT_ERROR_CHARS: usize = 300;

/// The name that selects the deterministic scripted model.
///
/// This is the only value `daemon.executor_model` accepts, because no live provider adapter path
/// exists yet: `P2-003` shipped the OpenAI-compatible adapter with no credentials, and selecting it
/// here would need provider coordinates that must not live in a plaintext configuration file.
pub const SCRIPTED_MODEL_NAME: &str = "scripted";

/// The answer the scripted executor produces.
///
/// A fixed string rather than generated text, and it says what it is. A deterministic model cannot
/// answer a question, and a placeholder that read like an answer would make a misconfigured daemon
/// look like a working one.
const SCRIPTED_ANSWER: &str = "No language model is configured, so this run was answered by the \
deterministic scripted model. Set daemon.executor_model once a provider adapter is available.";

/// Why an executor could not be built.
#[derive(Clone, Debug, Eq, thiserror::Error, PartialEq)]
pub enum ExecutorBuildError {
    /// The configured model name is not one this build implements.
    #[error("the configured executor model is not implemented by this build")]
    UnknownModel,
}

/// The model an executor drives, selected by name at daemon start.
///
/// Holds the port rather than a concrete adapter so adding a provider means adding a name here and
/// nothing else: the executor itself never learns which model it is driving.
pub struct Executor {
    model: Box<dyn ModelGateway>,
}

impl Executor {
    /// Builds an executor for a configured model name.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutorBuildError::UnknownModel`] for a name this build does not implement. The
    /// daemon refuses to start rather than accepting the configuration and then executing nothing,
    /// which is the failure a typo would otherwise produce silently.
    pub fn build(name: &str) -> Result<Self, ExecutorBuildError> {
        match name {
            SCRIPTED_MODEL_NAME => {
                let model = jarvis_models::scripted(
                    jarvis_models::SCRIPTED_PROVIDER,
                    "scripted-small",
                    vec![jarvis_models::Turn::answer(SCRIPTED_ANSWER)],
                )
                .map_err(|_| ExecutorBuildError::UnknownModel)?;
                Ok(Self {
                    model: Box::new(model),
                })
            }
            _ => Err(ExecutorBuildError::UnknownModel),
        }
    }

    /// Returns the model gateway the executor drives.
    #[must_use]
    pub fn model(&self) -> &dyn ModelGateway {
        self.model.as_ref()
    }
}

/// Drives one run to a terminal state.
///
/// Returns the settled run, or the storage failure that ended the attempt. A provider failure is
/// **not** an `Err`: it settles the run as failed and is returned as `Ok(settled)`, because the
/// run's outcome is data and only the persistence is the caller's problem.
///
/// # Errors
///
/// Returns [`DatabaseError`] when a run event or the settlement cannot be persisted, which leaves
/// the run open and is therefore a real failure rather than a run outcome.
pub async fn execute_run(
    database: &Arc<SqliteDatabase>,
    model: &dyn ModelGateway,
    run_id: &str,
) -> Result<StoredRun, DatabaseError> {
    let run = find_run(database, run_id).await?;

    // A run that already settled is returned as-is rather than restarted. Re-executing it would
    // append events to a stream that is provably closed, and the append would be refused anyway.
    if run.state().is_terminal() {
        return Ok(run);
    }

    let model_id = ModelId::new("scripted-small")
        .map_err(|_| DatabaseError::InvalidRunRequest { field: "model" })?;

    let mut calls = 0_u32;
    let mut current = run;
    let correlation_id = CorrelationId::new();

    loop {
        // A settlement ends the loop. Without this guard, a run that `fail`, `complete`, or
        // `check_cancellation` just settled re-enters the match below and is settled a second
        // time, which the domain refuses. That surfaced as
        // `TerminalStateImmutable { from: Failed }` rather than as anything about settlement
        // ordering, which is why it is worth a comment.
        if current.state().is_terminal() {
            return Ok(current);
        }

        current = match current.state() {
            RunState::Received => {
                current = advance(
                    database,
                    &current,
                    RunState::ContextBuilding,
                    RunEventKind::StateChanged,
                    Some("assembling context"),
                    r#"{"state":"context_building"}"#,
                    correlation_id,
                )
                .await?;
                current
            }
            RunState::ContextBuilding => {
                current = assemble_and_record(database, model, &current, &model_id, correlation_id)
                    .await?;
                current
            }
            RunState::Planning => {
                // One planned step: call the model. Recorded as a transition rather than skipped so
                // the stream explains the sequence the run actually took.
                current = advance(
                    database,
                    &current,
                    RunState::Executing,
                    RunEventKind::StateChanged,
                    Some("calling the model"),
                    r#"{"state":"executing"}"#,
                    correlation_id,
                )
                .await?;
                current
            }
            RunState::Executing => {
                calls += 1;
                if calls > MAX_MODEL_CALLS {
                    return fail(
                        database,
                        &current,
                        RunErrorCode::new("too_many_model_calls").map_err(|_| {
                            DatabaseError::InvalidRunRequest {
                                field: "error_code",
                            }
                        })?,
                        "the run exceeded its model call budget",
                        correlation_id,
                    )
                    .await;
                }
                current = generate(database, model, &current, &model_id, correlation_id).await?;
                current
            }
            RunState::Observing => {
                current = advance(
                    database,
                    &current,
                    RunState::Responding,
                    RunEventKind::StateChanged,
                    Some("producing the answer"),
                    r#"{"state":"responding"}"#,
                    correlation_id,
                )
                .await?;
                current
            }
            RunState::Responding => {
                return complete(database, &current, correlation_id).await;
            }
            // `AwaitingApproval` and every terminal state are not reachable in this slice: nothing
            // requests approval yet, and a terminal state is returned above. Reaching here would
            // mean the state machine moved somewhere this executor does not drive, so it is
            // reported rather than guessed at.
            other => {
                return fail(
                    database,
                    &current,
                    RunErrorCode::new("unexpected_state").map_err(|_| {
                        DatabaseError::InvalidRunRequest {
                            field: "error_code",
                        }
                    })?,
                    &format!("the run reached a state the executor does not handle: {other}"),
                    correlation_id,
                )
                .await;
            }
        };

        current = check_cancellation(database, &current, correlation_id).await?;
    }
}

/// Re-reads the run and settles it `cancelled` when a cancellation was requested.
///
/// Re-read rather than inspected from the value the loop holds, because a cancellation is recorded
/// by a *different* request while this one is in flight, so the value held here cannot see it. The
/// settle uses the freshly read state and version for the same reason.
async fn check_cancellation(
    database: &Arc<SqliteDatabase>,
    run: &StoredRun,
    correlation_id: CorrelationId,
) -> Result<StoredRun, DatabaseError> {
    let observed = find_run(database, run.id()).await?;
    if observed.cancellation_requested_at().is_none() || observed.state().is_terminal() {
        return Ok(observed);
    }

    settle(
        database,
        &observed,
        RunTransition::cancelled(),
        RunEventKind::RunCancelled,
        Some("the run was cancelled"),
        r#"{"outcome":"cancelled"}"#,
        correlation_id,
    )
    .await
}

/// Assembles the run's context and records what was included and excluded.
///
/// The manifest is recorded because it is audit evidence: `docs/architecture/memory-and-context.md`
/// requires the user be able to ask why something was used. Recording the counts now means the
/// answer exists before retrieval arrives, rather than being retrofitted from a manifest nobody
/// stored.
async fn assemble_and_record(
    database: &Arc<SqliteDatabase>,
    model: &dyn ModelGateway,
    run: &StoredRun,
    model_id: &ModelId,
    correlation_id: CorrelationId,
) -> Result<StoredRun, DatabaseError> {
    let budget = ContextBudget::new(
        TOTAL_CONTEXT_TOKENS,
        RESERVED_POLICY_TOKENS,
        RESERVED_USER_INTENT_TOKENS,
        PER_SOURCE_CAP_TOKENS,
    )
    .map_err(|_| DatabaseError::InvalidRunRequest {
        field: "context_budget",
    })?;

    let policy = ContextItem::new(
        ContextSource::new(ContextSourceKind::IdentityPolicy, "policy:native-v1")
            .map_err(|_| DatabaseError::InvalidRunRequest { field: "policy" })?,
        ContextTrust::Authoritative,
        Sensitivity::Public,
        ContextPriority::Required,
        estimate_tokens(SYSTEM_POLICY),
        InclusionReason::ReservedPolicy,
        false,
    )
    .map_err(|_| DatabaseError::InvalidRunRequest { field: "policy" })?;

    let objective = ContextItem::new(
        ContextSource::new(
            ContextSourceKind::CurrentInput,
            objective_reference(run.objective()),
        )
        .map_err(|_| DatabaseError::InvalidRunRequest { field: "objective" })?,
        ContextTrust::User,
        Sensitivity::Internal,
        ContextPriority::Required,
        estimate_tokens(run.objective()),
        InclusionReason::CurrentUserIntent,
        false,
    )
    .map_err(|_| DatabaseError::InvalidRunRequest { field: "objective" })?;

    // The destination ceiling is the model's **placement**, which is a privacy input rather than a
    // label, read from the adapter instead of assumed. A local model never leaves the machine, so
    // it may receive anything; a remote or unprobed model may not receive Confidential content.
    let ceiling = destination_ceiling(model, model_id).await;

    let manifest = assemble_context(vec![policy, objective], budget, ceiling)
        .map_err(|_| DatabaseError::InvalidRunRequest { field: "context" })?;

    let payload = format!(
        r#"{{"included":{},"excluded":{},"used_tokens":{},"instruction_tokens":{},"untrusted_tokens":{}}}"#,
        manifest.included().len(),
        manifest.excluded().len(),
        manifest.used_tokens(),
        manifest.instruction_tokens(),
        manifest.untrusted_tokens(),
    );

    append(
        database,
        run,
        RunEventKind::ActivityUpdated,
        Some("context assembled"),
        &payload,
        correlation_id,
    )
    .await?;

    // The manifest is recorded, then the run advances. The transition is separate so its own event
    // carries the state change, keeping "what was assembled" and "what state the run is in"
    // distinguishable in the stream rather than fused into one payload.
    advance(
        database,
        run,
        RunState::Planning,
        RunEventKind::StateChanged,
        Some("planning"),
        r#"{"state":"planning"}"#,
        correlation_id,
    )
    .await
}

/// Returns the most sensitive content the configured model may receive.
///
/// Read from the adapter's declared capabilities rather than assumed, because the placement is
/// what decides whether private content may leave the machine. A capability read that fails is
/// treated as remote: assuming local would permit a leak on an unverified assumption.
///
/// This is also what keeps the assembled context and the request consistent. Assembling under a
/// ceiling that excludes the objective and then sending the objective anyway would be a privacy
/// defect that no test of the manifest alone could see.
async fn destination_ceiling(model: &dyn ModelGateway, model_id: &ModelId) -> Sensitivity {
    match model.capabilities(model_id).await {
        Ok(capabilities) if capabilities.placement() == Placement::Local => Sensitivity::Restricted,
        Ok(_) | Err(_) => Sensitivity::Internal,
    }
}

/// A stream failure that must settle the run rather than propagate as a storage error.
struct StreamFailure {
    code: RunErrorCode,
    message: String,
}

/// Consumes a validated model stream, appending one event per text fragment.
///
/// Returns the validated summary and the concatenated answer, or the failure that must settle the
/// run. Kept separate from [`generate`] so the streaming rules — sequence validation, one event per
/// fragment, an incomplete stream being ambiguous rather than successful — are readable as one unit.
///
/// # Errors
///
/// Returns [`DatabaseError`] only when an event cannot be persisted. A model-side failure is
/// returned as `Ok(Err(..))` because it is a run outcome, not a persistence problem.
async fn consume_stream(
    database: &Arc<SqliteDatabase>,
    run: &StoredRun,
    stream: jarvis_models::ModelStream,
    correlation_id: CorrelationId,
) -> Result<Result<(jarvis_models::StreamSummary, String), StreamFailure>, DatabaseError> {
    let mut stream = stream;
    let mut validator = StreamValidator::new();
    let mut text = String::new();

    while let Some(item) = futures_util::StreamExt::next(&mut stream).await {
        match item {
            Ok(envelope) => {
                if validator.accept(envelope.clone()).is_err() {
                    return Ok(Err(StreamFailure {
                        code: RunErrorCode::new("stream_sequence_invalid").map_err(|_| {
                            DatabaseError::InvalidRunEventRequest {
                                field: "error_code",
                            }
                        })?,
                        message: "the model stream violated its sequence contract".to_owned(),
                    }));
                }
                if let StreamEvent::TextDelta { text: fragment } = envelope.event() {
                    text.push_str(fragment);
                    // Each fragment is its own durable event, so a client that reconnects mid-answer
                    // replays the fragments it did not see rather than the whole answer.
                    if !fragment.is_empty() {
                        append(
                            database,
                            run,
                            RunEventKind::OutputDelta,
                            None,
                            &format!(r#"{{"text":{}}}"#, json_string(fragment)),
                            correlation_id,
                        )
                        .await?;
                    }
                }
            }
            Err(error) => {
                return Ok(Err(StreamFailure {
                    code: model_error_code(error.kind()),
                    message: format!("the model stream failed: {}", error.message()),
                }));
            }
        }
    }

    match validator.finish() {
        Ok(summary) => Ok(Ok((summary, text))),
        // A stream that ended without a terminal event is ambiguous, not successful. Reporting it
        // as completed would present a truncated answer as a whole one.
        Err(_) => Ok(Err(StreamFailure {
            code: RunErrorCode::new("stream_incomplete").map_err(|_| {
                DatabaseError::InvalidRunEventRequest {
                    field: "error_code",
                }
            })?,
            message: "the model stream ended without a terminal event".to_owned(),
        })),
    }
}

/// Calls the model once and records the answer or the failure.
async fn generate(
    database: &Arc<SqliteDatabase>,
    model: &dyn ModelGateway,
    run: &StoredRun,
    model_id: &ModelId,
    correlation_id: CorrelationId,
) -> Result<StoredRun, DatabaseError> {
    let request = ChatRequest::new(
        model_id.clone(),
        vec![
            ChatMessage::system(SYSTEM_POLICY),
            ChatMessage::user(run.objective()),
        ],
        correlation_id,
    );

    let stream = match model.stream(request).await {
        Ok(stream) => stream,
        Err(error) => {
            return fail(
                database,
                run,
                model_error_code(error.kind()),
                &format!("the model call failed: {}", error.message()),
                correlation_id,
            )
            .await;
        }
    };

    // The stream is validated rather than accumulated blindly, so a gap or a late event is a
    // reported failure. A sequence defect means events were lost, and a lost event could be the
    // terminal one — the difference between a complete answer and a truncated one.
    let (summary, text) = match consume_stream(database, run, stream, correlation_id).await? {
        Ok(outcome) => outcome,
        Err(failure) => {
            return fail(
                database,
                run,
                failure.code,
                &failure.message,
                correlation_id,
            )
            .await;
        }
    };

    // A refusal is a provider decision, and recording it as a failure is what distinguishes "the
    // model would not answer" from "the model had nothing to say".
    if summary
        .finish_reason()
        .is_some_and(FinishReason::is_refusal)
    {
        return fail(
            database,
            run,
            RunErrorCode::new("content_refusal").map_err(|_| DatabaseError::InvalidRunRequest {
                field: "error_code",
            })?,
            "the model refused to answer",
            correlation_id,
        )
        .await;
    }

    append(
        database,
        run,
        RunEventKind::OutputCompleted,
        None,
        &format!(
            r#"{{"text":{},"chars":{}}}"#,
            json_string(&text),
            text.chars().count()
        ),
        correlation_id,
    )
    .await?;

    // Usage is recorded because a streamed run that omits it leaves a cost record missing with no
    // sign that it is missing.
    if let Some(usage) = summary.usage() {
        append(
            database,
            run,
            RunEventKind::UsageUpdated,
            None,
            &format!(
                r#"{{"input_tokens":{},"output_tokens":{},"cached_input_tokens":{}}}"#,
                usage.input_tokens(),
                usage.output_tokens(),
                usage.cached_input_tokens(),
            ),
            correlation_id,
        )
        .await?;
    }

    advance(
        database,
        run,
        RunState::Observing,
        RunEventKind::StateChanged,
        Some("interpreting the result"),
        r#"{"state":"observing"}"#,
        correlation_id,
    )
    .await
}

/// Settles a responding run as completed.
///
/// The answer is stored as a transcript message **before** the settlement, so a completed
/// conversation is one whose transcript holds both sides. Stored after the settle it could be lost
/// to a crash between the two writes, leaving a run that answered a question the transcript does not
/// contain.
async fn complete(
    database: &Arc<SqliteDatabase>,
    run: &StoredRun,
    correlation_id: CorrelationId,
) -> Result<StoredRun, DatabaseError> {
    if let Some(answer) = last_answer(database, run).await? {
        let message = NewMessage::assistant(answer, Sensitivity::Internal)
            .map_err(|_| DatabaseError::InvalidMessageRequest { field: "content" })?;
        jarvis_storage::append_message(
            database,
            run.session_id(),
            Some(run.id()),
            &message,
            correlation_id,
            UtcTimestamp::now(&SystemClock),
        )
        .await?;
    }

    settle(
        database,
        run,
        RunTransition::completed(),
        RunEventKind::RunCompleted,
        Some("the run completed"),
        r#"{"outcome":"succeeded"}"#,
        correlation_id,
    )
    .await
}

/// Reads the run's completed answer from its own event stream.
///
/// Read back rather than threaded through the loop, so the stored transcript and the stored events
/// cannot disagree: the message is built from the event that a client already receives, not from a
/// second copy of the text held in memory.
async fn last_answer(
    database: &Arc<SqliteDatabase>,
    run: &StoredRun,
) -> Result<Option<String>, DatabaseError> {
    let events = jarvis_storage::read_run_events(
        database,
        run.id(),
        jarvis_core::ReplayRequest::new(jarvis_core::RunEventSequence::first(), MAX_EVENTS_READ)
            .map_err(|_| DatabaseError::InvalidRunEventRequest { field: "replay" })?,
    )
    .await?;

    for event in events.iter().rev() {
        if event.kind() != RunEventKind::OutputCompleted {
            continue;
        }
        let payload: serde_json::Value = serde_json::from_str(event.payload())
            .map_err(|_| DatabaseError::StoredRunEventInvalid { field: "payload" })?;
        if let Some(text) = payload.get("text").and_then(serde_json::Value::as_str) {
            return Ok(Some(text.to_owned()));
        }
    }
    Ok(None)
}

/// Settles a run as failed with a bounded reason.
async fn fail(
    database: &Arc<SqliteDatabase>,
    run: &StoredRun,
    code: RunErrorCode,
    message: &str,
    correlation_id: CorrelationId,
) -> Result<StoredRun, DatabaseError> {
    let payload = format!(
        r#"{{"outcome":"failed","error_code":{},"message":{}}}"#,
        json_string(code.as_str()),
        json_string(&truncate(message, MAX_EVENT_ERROR_CHARS)),
    );
    settle(
        database,
        run,
        RunTransition::failed(code),
        RunEventKind::RunFailed,
        Some("the run failed"),
        &payload,
        correlation_id,
    )
    .await
}

/// Appends an event to a run that is still open.
async fn append(
    database: &Arc<SqliteDatabase>,
    run: &StoredRun,
    kind: RunEventKind,
    summary: Option<&str>,
    payload: &str,
    correlation_id: CorrelationId,
) -> Result<(), DatabaseError> {
    let summary = match summary {
        Some(text) => Some(
            EventSummary::new(text)
                .map_err(|_| DatabaseError::InvalidRunEventRequest { field: "summary" })?,
        ),
        None => None,
    };
    let event = NewRunEvent::new(
        jarvis_core::RunId::new().to_string(),
        run.id(),
        kind,
        summary,
        RunEventPayload::new(payload)
            .map_err(|_| DatabaseError::InvalidRunEventRequest { field: "payload" })?,
        correlation_id,
        UtcTimestamp::now(&SystemClock),
    )?;
    append_run_event(database, &event).await.map(|_| ())
}

/// Advances a run and records the transition as an event.
///
/// Re-reads the run first, for the same reason [`settle`] does: the row version is also advanced by
/// writers that leave the state alone, so a cancellation requested during a model call would
/// otherwise turn this guard into a spurious `RunConflict`.
async fn advance(
    database: &Arc<SqliteDatabase>,
    run: &StoredRun,
    to: RunState,
    kind: RunEventKind,
    summary: Option<&str>,
    payload: &str,
    correlation_id: CorrelationId,
) -> Result<StoredRun, DatabaseError> {
    let current = find_run(database, run.id()).await?;
    // A run that settled while this step was prepared must not move again, and a cancellation that
    // arrived is handled by the caller's cancellation check rather than by forcing the step.
    if current.state().is_terminal() {
        return Ok(current);
    }

    let transition = RunTransition::new(to, to.required_outcome(), None).map_err(|_| {
        DatabaseError::InvalidRunRequest {
            field: "transition",
        }
    })?;
    append(database, &current, kind, summary, payload, correlation_id).await?;
    jarvis_storage::transition_run(
        database,
        current.id(),
        current.expectation(),
        &transition,
        UtcTimestamp::now(&SystemClock),
    )
    .await
}

/// Settles a run and records the terminal event in the same transaction.
///
/// Re-reads the run first rather than trusting the caller's copy, because the row version is also
/// bumped by writers that do **not** change the state. A cancellation requested while a model call
/// was in flight is exactly that: it stamps `cancellation_requested_at` and advances `version`
/// while leaving `state` terminal-free. Trusting a stale version would then turn an honest
/// cancellation into `RunConflict`, which reads as a concurrency bug rather than as a run being
/// cancelled.
async fn settle(
    database: &Arc<SqliteDatabase>,
    run: &StoredRun,
    transition: RunTransition,
    kind: RunEventKind,
    summary: Option<&str>,
    payload: &str,
    correlation_id: CorrelationId,
) -> Result<StoredRun, DatabaseError> {
    let current = find_run(database, run.id()).await?;
    // A run that another writer already settled is returned as-is. Settling again is refused by
    // the domain, and the honest answer is the state the row actually holds.
    if current.state().is_terminal() {
        return Ok(current);
    }

    let summary = match summary {
        Some(text) => Some(
            EventSummary::new(text)
                .map_err(|_| DatabaseError::InvalidRunEventRequest { field: "summary" })?,
        ),
        None => None,
    };
    let event = NewRunEvent::new(
        jarvis_core::RunId::new().to_string(),
        current.id(),
        kind,
        summary,
        RunEventPayload::new(payload)
            .map_err(|_| DatabaseError::InvalidRunEventRequest { field: "payload" })?,
        correlation_id,
        UtcTimestamp::now(&SystemClock),
    )?;
    let settlement = TerminalTransition::new(current.expectation(), transition, &event)?;
    settle_run(database, &settlement).await
}

/// Maps a normalized model error kind onto the run's stored failure code.
///
/// A closed mapping rather than a formatted string, so the stored code is one of a known set and a
/// client can branch on it. The provider's own message is recorded in the event payload, not here.
fn model_error_code(kind: jarvis_models::ModelErrorKind) -> RunErrorCode {
    let value = match kind {
        jarvis_models::ModelErrorKind::Authentication => "model_authentication",
        jarvis_models::ModelErrorKind::Authorization => "model_authorization",
        jarvis_models::ModelErrorKind::ContentRefusal => "content_refusal",
        jarvis_models::ModelErrorKind::RateLimited => "model_rate_limited",
        jarvis_models::ModelErrorKind::Overloaded => "model_overloaded",
        jarvis_models::ModelErrorKind::Transient => "model_transient",
        jarvis_models::ModelErrorKind::QuotaExhausted => "model_quota_exhausted",
        jarvis_models::ModelErrorKind::ContextOverflow => "model_context_overflow",
        jarvis_models::ModelErrorKind::ModelNotFound => "model_not_found",
        jarvis_models::ModelErrorKind::Timeout => "model_timeout",
        jarvis_models::ModelErrorKind::Cancelled => "model_cancelled",
        jarvis_models::ModelErrorKind::Incomplete => "model_incomplete",
        jarvis_models::ModelErrorKind::InvalidRequest => "model_request_invalid",
        jarvis_models::ModelErrorKind::MalformedResponse => "model_response_malformed",
        jarvis_models::ModelErrorKind::Internal => "model_internal",
    };
    // Every value above is lower-case ASCII with underscores, so this cannot fail; the fallback
    // exists because `new` is fallible and a panic here would replace a run's honest failure with
    // a crash.
    RunErrorCode::new(value).unwrap_or_else(|_| {
        RunErrorCode::new("model_error")
            .unwrap_or_else(|_| unreachable!("'model_error' is portable"))
    })
}

/// Estimates tokens for a text, in UTF-8 bytes.
///
/// A deliberately crude bound rather than a tokenizer. `P2-009` needs the reservation arithmetic to
/// be exercised, and a byte count is an over-estimate for English text, so the budget errs toward
/// refusing content rather than toward exceeding the window. A real tokenizer arrives with the
/// provider capability work.
fn estimate_tokens(text: &str) -> u32 {
    // At least one, because the context contract rejects a zero estimate and an empty objective is
    // already refused by the storage schema.
    u32::try_from(text.len()).unwrap_or(u32::MAX).max(1)
}

/// Builds a bounded source reference for an objective.
fn objective_reference(objective: &str) -> String {
    let bounded = truncate(objective, MAX_OBJECTIVE_REFERENCE_CHARS);
    format!("objective:{bounded}")
}

/// Truncates text to a character bound, preserving the beginning.
fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    text.chars().take(limit).collect::<String>() + "…"
}

/// Encodes a string as a JSON string literal.
///
/// `serde_json` on a `&str` rather than manual escaping, so a quote or a newline in a provider
/// message cannot produce a payload the reader cannot parse. The fallback is unreachable for a
/// string, and returning an empty literal is safer than panicking inside a run.
fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| String::from("\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    use jarvis_models::{ScriptedModel, Turn, scripted};
    use jarvis_storage::{API_SESSION_CHANNEL, StartRunInput, start_run};

    /// A temporary profile directory holding a migrated database.
    struct TempProfile(std::path::PathBuf);

    impl TempProfile {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "jarvis-executor-{}-{}",
                std::process::id(),
                jarvis_core::RunId::new()
            ));
            std::fs::create_dir_all(&path)
                .unwrap_or_else(|error| panic!("create temp profile: {error}"));
            Self(path)
        }

        fn database_path(&self) -> std::path::PathBuf {
            self.0.join(jarvis_storage::DEFAULT_DATABASE_FILENAME)
        }
    }

    impl Drop for TempProfile {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Opens a real migrated database, so the seeded identity rows are present.
    async fn database() -> (TempProfile, Arc<SqliteDatabase>) {
        let profile = TempProfile::new();
        let database = SqliteDatabase::open(&profile.database_path())
            .await
            .unwrap_or_else(|error| panic!("open fixture database: {error}"));
        (profile, Arc::new(database))
    }

    /// Starts a run through the real start path, so its first event exists as production writes it.
    async fn start(database: &Arc<SqliteDatabase>, objective: &str) -> StoredRun {
        let identity = jarvis_storage::load_local_identity(database)
            .await
            .unwrap_or_else(|error| panic!("the fixture must have a seeded identity: {error}"));
        let input = StartRunInput::new(
            jarvis_core::SessionId::new().to_string(),
            jarvis_core::RunId::new().to_string(),
            jarvis_core::RequestId::new().to_string(),
            identity.workspace_id(),
            identity.user_id(),
            objective,
            API_SESSION_CHANNEL,
            CorrelationId::new(),
            UtcTimestamp::now(&SystemClock),
        )
        .unwrap_or_else(|error| panic!("start input: {error}"));
        let started = start_run(database, &input)
            .await
            .unwrap_or_else(|error| panic!("start run: {error}"));
        // Read the run back through the real read path rather than reconstructing it from the
        // start reply, so a divergence between the written and read representations is caught here
        // rather than being hidden by the fixture.
        find_run(database, started.run_id())
            .await
            .unwrap_or_else(|error| panic!("read the started run: {error}"))
    }

    fn model(turns: Vec<Turn>) -> ScriptedModel {
        scripted("scripted", "scripted-small", turns)
            .unwrap_or_else(|error| panic!("scripted model: {error}"))
    }

    async fn events(database: &Arc<SqliteDatabase>, run_id: &str) -> Vec<RunEventKind> {
        jarvis_storage::read_run_events(
            database,
            run_id,
            jarvis_core::ReplayRequest::new(jarvis_core::RunEventSequence::first(), 100)
                .unwrap_or_else(|error| panic!("replay request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("read events: {error}"))
        .into_iter()
        .map(|event| event.kind())
        .collect()
    }

    /// The whole point of the slice: a run reaches a terminal state and streams its answer.
    #[tokio::test]
    async fn a_run_completes_and_records_its_answer() {
        let (_profile, database) = database().await;
        let run = start(&database, "summarise my inbox").await;
        assert_eq!(run.state(), RunState::Received);

        let settled = execute_run(
            &database,
            &model(vec![Turn::answer("Three unread.")]),
            run.id(),
        )
        .await
        .unwrap_or_else(|error| panic!("execute: {error}"));

        assert_eq!(settled.state(), RunState::Completed);
        assert_eq!(
            settled.terminal_outcome(),
            Some(jarvis_core::RunOutcome::Succeeded)
        );

        let kinds = events(&database, run.id()).await;
        assert!(kinds.contains(&RunEventKind::OutputDelta));
        assert!(kinds.contains(&RunEventKind::OutputCompleted));
        assert!(kinds.contains(&RunEventKind::UsageUpdated));
        assert_eq!(
            kinds.last(),
            Some(&RunEventKind::RunCompleted),
            "the last event must be the terminal one"
        );
        database.close().await;
    }

    /// A fragmented answer must produce one event per fragment, so a reconnect can replay from a
    /// position rather than re-receiving the whole answer.
    #[tokio::test]
    async fn a_fragmented_answer_records_one_event_per_fragment() {
        let (_profile, database) = database().await;
        let run = start(&database, "count").await;

        let settled = execute_run(
            &database,
            &model(vec![Turn::streamed(&["One", ", two", ", three."])]),
            run.id(),
        )
        .await
        .unwrap_or_else(|error| panic!("execute: {error}"));
        assert_eq!(settled.state(), RunState::Completed);

        let kinds = events(&database, run.id()).await;
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| **kind == RunEventKind::OutputDelta)
                .count(),
            3,
            "each fragment is its own durable event"
        );
        database.close().await;
    }

    /// A provider failure settles the run as failed with the mapped code, and does not report
    /// success. `Err` is reserved for persistence failures.
    #[tokio::test]
    async fn a_provider_failure_settles_the_run_as_failed() {
        let (_profile, database) = database().await;
        let run = start(&database, "anything").await;

        let settled = execute_run(&database, &model(vec![Turn::refusal()]), run.id())
            .await
            .unwrap_or_else(|error| panic!("a provider failure must be a run outcome: {error:?}"));
        assert_eq!(settled.state(), RunState::Failed);
        assert_eq!(
            settled.error_code(),
            Some("content_refusal"),
            "the stored code must be the mapped provider category"
        );

        let kinds = events(&database, run.id()).await;
        assert_eq!(kinds.last(), Some(&RunEventKind::RunFailed));
        database.close().await;
    }

    /// A cancellation requested while the run is in flight settles it once, as `cancelled`.
    ///
    /// The request is made from another task while the model call is parked, so the loop must
    /// re-read the run to see it — a value cached before the call cannot.
    #[tokio::test]
    async fn a_cancellation_requested_in_flight_settles_the_run_once() {
        let (_profile, database) = database().await;
        let run = start(&database, "slow").await;

        let slow = model(vec![Turn::slow(400, Turn::answer("too late"))]);
        let executor_database = Arc::clone(&database);
        let run_id = run.id().to_owned();
        let handle =
            tokio::spawn(async move { execute_run(&executor_database, &slow, &run_id).await });

        // Request cancellation through the real path while the model call is parked.
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        let observed = find_run(&database, run.id())
            .await
            .unwrap_or_else(|error| panic!("read: {error}"));
        jarvis_storage::request_run_cancellation(
            &database,
            run.id(),
            observed.expectation(),
            UtcTimestamp::now(&SystemClock),
        )
        .await
        .unwrap_or_else(|error| panic!("request cancellation: {error}"));

        let settled = handle
            .await
            .unwrap_or_else(|error| panic!("join: {error}"))
            .unwrap_or_else(|error| panic!("execute: {error}"));

        assert_eq!(settled.state(), RunState::Cancelled);
        assert_eq!(
            settled.terminal_outcome(),
            Some(jarvis_core::RunOutcome::Cancelled)
        );

        let kinds = events(&database, run.id()).await;
        assert_eq!(
            kinds.iter().filter(|kind| kind.is_terminal()).count(),
            1,
            "cancellation must settle the run exactly once"
        );
        database.close().await;
    }

    /// Executing an already-settled run returns it unchanged, because its stream is provably closed.
    #[tokio::test]
    async fn an_already_settled_run_is_not_restarted() {
        let (_profile, database) = database().await;
        let run = start(&database, "once").await;
        let first = execute_run(&database, &model(vec![Turn::answer("done")]), run.id())
            .await
            .unwrap_or_else(|error| panic!("execute: {error}"));
        let before = events(&database, run.id()).await.len();

        let second = execute_run(&database, &model(vec![Turn::answer("again")]), run.id())
            .await
            .unwrap_or_else(|error| panic!("execute: {error}"));

        assert_eq!(second.state(), first.state());
        assert_eq!(
            events(&database, run.id()).await.len(),
            before,
            "a settled run must emit nothing more"
        );
        database.close().await;
    }

    /// A completed run stores BOTH sides of the conversation, so A03's "never loses an accepted user
    /// message" is met by a transcript that actually exists rather than by a table nobody writes.
    ///
    /// Three assertions, because each can fail independently: the user's question is present, the
    /// model's answer is present, and they are in order.
    #[tokio::test]
    async fn a_completed_run_stores_the_question_and_the_answer() {
        let (_profile, database) = database().await;
        let run = start(&database, "what is in my inbox").await;

        execute_run(
            &database,
            &model(vec![Turn::answer("Three unread.")]),
            run.id(),
        )
        .await
        .unwrap_or_else(|error| panic!("execute: {error}"));

        let transcript = jarvis_storage::read_messages(&database, run.session_id(), 50)
            .await
            .unwrap_or_else(|error| panic!("read transcript: {error}"));

        assert_eq!(
            transcript.len(),
            2,
            "a completed conversation has a question and an answer"
        );
        assert_eq!(transcript[0].content(), "what is in my inbox");
        assert_eq!(transcript[0].role(), jarvis_core::MessageRole::User);
        assert_eq!(transcript[0].sequence(), 0);
        assert_eq!(transcript[1].content(), "Three unread.");
        assert_eq!(transcript[1].role(), jarvis_core::MessageRole::Assistant);
        assert_eq!(
            transcript[1].run_id(),
            Some(run.id()),
            "the answer names the run that produced it"
        );
        database.close().await;
    }

    /// A failed run stores the question but no answer, because there is no answer to store.
    #[tokio::test]
    async fn a_failed_run_does_not_store_an_answer() {
        let (_profile, database) = database().await;
        let run = start(&database, "anything").await;

        execute_run(&database, &model(vec![Turn::refusal()]), run.id())
            .await
            .unwrap_or_else(|error| panic!("execute: {error}"));

        let transcript = jarvis_storage::read_messages(&database, run.session_id(), 50)
            .await
            .unwrap_or_else(|error| panic!("read transcript: {error}"));
        assert_eq!(
            transcript.len(),
            1,
            "a failed run must not record an answer it did not produce"
        );
        assert_eq!(transcript[0].role(), jarvis_core::MessageRole::User);
        database.close().await;
    }

    /// The context manifest is recorded, because it is the evidence a user can be shown.
    #[tokio::test]
    async fn the_context_manifest_is_recorded() {
        let (_profile, database) = database().await;
        let run = start(&database, "explain").await;
        execute_run(&database, &model(vec![Turn::answer("ok")]), run.id())
            .await
            .unwrap_or_else(|error| panic!("execute: {error}"));

        let events = jarvis_storage::read_run_events(
            &database,
            run.id(),
            jarvis_core::ReplayRequest::new(jarvis_core::RunEventSequence::first(), 100)
                .unwrap_or_else(|error| panic!("replay request: {error}")),
        )
        .await
        .unwrap_or_else(|error| panic!("read events: {error}"));

        let manifest = events
            .iter()
            .find(|event| event.kind() == RunEventKind::ActivityUpdated)
            .unwrap_or_else(|| panic!("the context manifest must be recorded"));
        let payload: serde_json::Value = serde_json::from_str(manifest.payload())
            .unwrap_or_else(|error| panic!("the manifest must be JSON: {error}"));
        assert_eq!(payload["included"], 2, "policy and the objective");
        assert_eq!(payload["excluded"], 0);
        database.close().await;
    }

    /// The estimator must never return zero, because the context contract rejects a zero estimate
    /// and an objective is always non-empty.
    #[test]
    fn the_token_estimate_is_never_zero() {
        assert!(estimate_tokens("") >= 1);
        assert!(estimate_tokens("hello") >= 5);
    }

    /// Truncation is by characters, so a multi-byte objective cannot be cut mid-character.
    #[test]
    fn truncation_is_by_characters_not_bytes() {
        let text = "é".repeat(10);
        let truncated = truncate(&text, 3);
        assert_eq!(
            truncated.chars().count(),
            4,
            "three characters plus the ellipsis"
        );
        assert!(truncated.starts_with("ééé"));
    }
}
