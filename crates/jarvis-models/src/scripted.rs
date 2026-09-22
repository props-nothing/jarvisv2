//! A deterministic, scripted model adapter.
//!
//! `P2-009` needs a model that behaves the same way on every run so state, retry, stream, and
//! cancellation behavior can be tested without a network, a credential, or a cost. This module is
//! that adapter, and it implements the **same** [`ModelGateway`] port as the real provider adapter,
//! so a consumer cannot tell a scripted run from a live one except by what it was told to do.
//!
//! # Why this lives in `jarvis-models`
//!
//! It is a provider adapter, not a test-only double, because two things need it: the test suite, and
//! the first end-to-end run of the executor. `docs/development/definition-of-done.md` forbids
//! describing a mock-only path as complete, so this is deliberately **not** a mock: it produces real
//! [`StreamEnvelope`] sequences, real usage, and real normalized errors, and a consumer consumes it
//! through the production port.
//!
//! It is never selected in a configuration that could reach a user by accident: it is not
//! constructible from configuration and must be built explicitly in code, which is what makes it safe
//! to ship rather than gate behind `#[cfg(test)]`. A `#[cfg(test)]` adapter would also be invisible to
//! anything outside this crate — including the daemon — which is exactly where it is needed most.
//!
//! # Determinism is the whole point
//!
//! Every method takes no wall clock, performs no I/O, and consults no environment. A scripted turn
//! either yields the events it was given or the error it was given, in the order given. That is what
//! lets a test assert a *sequence* rather than a tolerance, and what makes a restart test reproducible
//! from a fixed input rather than from timing.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::capability::{ModelCapabilities, Placement, Support};
use crate::error::{ModelError, ModelErrorKind};
use crate::identity::{ModelId, ProviderId};
use crate::port::{ModelGateway, ModelStream, ProviderHealth, ProviderStatus};
use crate::request::{ChatMessage, ChatRequest};
use crate::response::{ChatResponse, FinishReason, OutputContent};
use crate::stream::{StreamEnvelope, StreamEvent};
use crate::usage::TokenUsage;

/// The provider identifier a scripted adapter reports.
pub const SCRIPTED_PROVIDER: &str = "scripted";

/// What one scripted turn does.
#[derive(Clone, Debug)]
pub enum Turn {
    /// Produce a complete answer made of these text fragments, then finish.
    ///
    /// Fragments are emitted as separate [`StreamEvent::TextDelta`] events so a consumer sees a
    /// multi-chunk answer rather than one chunk, which is what a streamed response actually looks
    /// like and what a partial-output bug would hide behind.
    Answer {
        /// The text fragments, concatenated in order.
        fragments: Vec<String>,
        /// The stated reason generation stopped.
        reason: FinishReason,
        /// Usage to report, if any.
        usage: Option<TokenUsage>,
    },
    /// Fail with this normalized error.
    Fail(ModelErrorKind),
    /// Delay for this many milliseconds before answering, so a cancellation can arrive mid-turn.
    ///
    /// A delay is a real `tokio` sleep, so it yields to the runtime and a cancellation that is racing
    /// it can be observed. It is not a busy wait and it holds no lock.
    Slow {
        /// How long to wait before doing anything else.
        delay_ms: u64,
        /// What to do once the delay elapses.
        then: Box<Turn>,
    },
}

impl Turn {
    /// A plain answer with a `stop` finish reason and synthetic usage.
    #[must_use]
    pub fn answer(text: &str) -> Self {
        Self::Answer {
            fragments: vec![text.to_owned()],
            reason: FinishReason::Stop,
            usage: Some(TokenUsage::new(11, 7)),
        }
    }

    /// An answer delivered one fragmented character group at a time.
    #[must_use]
    pub fn streamed(fragments: &[&str]) -> Self {
        Self::Answer {
            fragments: fragments.iter().map(|part| (*part).to_owned()).collect(),
            reason: FinishReason::Stop,
            usage: Some(TokenUsage::new(11, 7)),
        }
    }

    /// A turn that fails with a retryable provider error.
    #[must_use]
    pub fn transient_failure() -> Self {
        Self::Fail(ModelErrorKind::Transient)
    }

    /// A turn that fails because the provider refused the output.
    #[must_use]
    pub fn refusal() -> Self {
        Self::Fail(ModelErrorKind::ContentRefusal)
    }

    /// A turn that waits before answering.
    #[must_use]
    pub fn slow(delay_ms: u64, then: Self) -> Self {
        Self::Slow {
            delay_ms,
            then: Box::new(then),
        }
    }
}

/// A cancellation signal a scripted turn observes.
///
/// A plain `AtomicBool` rather than a `CancellationToken`, so this adapter adds no dependency and a
/// test can fire it deterministically from another thread. It is checked *before* each awaited step
/// rather than only at the start, because a cancellation that arrives mid-turn is the case the
/// acceptance criteria name.
#[derive(Debug, Default)]
pub struct ScriptedCancellation(AtomicBool);

impl ScriptedCancellation {
    /// Creates a signal that has not fired.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fires the signal.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Reports whether the signal has fired.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A scripted model adapter that repeats a fixed sequence of turns.
///
/// # Why it repeats rather than runs once
///
/// A real provider answers every request. An adapter that ran out of script and started failing would
/// turn "the executor made one extra call" into a confusing error instead of a visible call count, and
/// the call count is one of the things `P2-009` must be able to assert. So the last turn repeats, and
/// the number of calls is observable through [`Self::calls`].
#[derive(Debug)]
pub struct ScriptedModel {
    provider: ProviderId,
    model: ModelId,
    turns: Vec<Turn>,
    cursor: Mutex<usize>,
    cancellation: Option<std::sync::Arc<ScriptedCancellation>>,
    capabilities: ModelCapabilities,
    /// The messages of every request served, in call order.
    ///
    /// Recorded because the *request* is what a multi-turn conversation is: an adapter that only
    /// returns turns cannot show whether history reached the model, so a test of "the earlier turns
    /// were replayed" would have nothing to assert against and would pass for a daemon that sent
    /// only the latest question. Recording the request is what makes that assertable.
    seen: Mutex<Vec<Vec<ChatMessage>>>,
}

impl ScriptedModel {
    /// Builds an adapter that serves `model`, using `provider` as its identifier.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] when either identifier is invalid, or when `turns` is empty (a script
    /// with no turns would answer nothing and report success).
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        turns: Vec<Turn>,
    ) -> Result<Self, ModelError> {
        if turns.is_empty() {
            return Err(ModelError::from_static(
                ModelErrorKind::Internal,
                "a scripted model requires at least one turn",
            ));
        }
        let provider = ProviderId::new(provider.into()).map_err(|_| {
            ModelError::from_static(ModelErrorKind::Internal, "the provider id is invalid")
        })?;
        let model = ModelId::new(model.into()).map_err(|_| {
            ModelError::from_static(ModelErrorKind::Internal, "the model id is invalid")
        })?;
        Ok(Self {
            provider,
            model,
            turns,
            cursor: Mutex::new(0),
            cancellation: None,
            seen: Mutex::new(Vec::new()),
            // A scripted model declares what it is: local, streaming, and usage-reporting. The
            // placement is `Local` because nothing leaves the process, which makes a
            // `Sensitivity::Internal` destination able to receive any content — the point of a
            // deterministic adapter that a private-content test can use without a network.
            capabilities: ModelCapabilities::unknown()
                .with_streaming(Support::Supported)
                .with_usage_on_stream(Support::Supported)
                .with_tool_calls(Support::Unsupported)
                .with_placement(Some(Placement::Local)),
        })
    }

    /// Attaches a cancellation signal the turns observe.
    #[must_use]
    pub fn with_cancellation(mut self, signal: std::sync::Arc<ScriptedCancellation>) -> Self {
        self.cancellation = Some(signal);
        self
    }

    /// Overrides the reported capabilities.
    #[must_use]
    pub const fn with_capabilities(mut self, capabilities: ModelCapabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    /// Returns the messages of every request this adapter has served, in call order.
    ///
    /// A conversation is a property of the *request*, not of the answer, so this is what makes
    /// "the earlier turns were replayed" a testable claim. Without it a daemon that sent only the
    /// latest question would satisfy every assertion about the answer.
    #[must_use]
    pub fn seen_messages(&self) -> Vec<Vec<ChatMessage>> {
        self.seen
            .lock()
            .map_or_else(|_| Vec::new(), |seen| seen.clone())
    }

    /// Records one request's messages.
    fn record(&self, request: &ChatRequest) {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(request.messages().to_vec());
        }
    }

    /// Returns how many turns have been served.
    ///
    /// One call per `complete` or `stream`, so a caller can assert that an operation made exactly the
    /// number of provider attempts it should have — which is how a runaway retry loop becomes visible.
    #[must_use]
    pub fn calls(&self) -> usize {
        self.cursor.lock().map_or(0, |cursor| *cursor)
    }

    /// Returns the turn for the next call, advancing the cursor.
    ///
    /// The final turn repeats forever, so a caller that makes one call too many sees a served turn
    /// rather than an out-of-script error. A poisoned lock is reported as an internal error rather
    /// than unwrapped, because a panicking test thread must not turn into a panicking adapter.
    fn next_turn(&self) -> Result<Turn, ModelError> {
        let mut cursor = self.cursor.lock().map_err(|_| {
            ModelError::from_static(ModelErrorKind::Internal, "the script cursor is poisoned")
        })?;
        let index = (*cursor).min(self.turns.len().saturating_sub(1));
        *cursor += 1;
        Ok(self.turns[index].clone())
    }

    /// Fails if the cancellation signal has fired.
    fn check_cancelled(&self) -> Result<(), ModelError> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(|signal| signal.is_cancelled())
        {
            return Err(ModelError::from_static(
                ModelErrorKind::Cancelled,
                "the model call was cancelled",
            ));
        }
        Ok(())
    }

    /// Resolves a turn into its answered text or its error, honoring a delay.
    async fn resolve(
        &self,
        turn: &Turn,
    ) -> Result<(Vec<String>, FinishReason, Option<TokenUsage>), ModelError> {
        match turn {
            Turn::Answer {
                fragments,
                reason,
                usage,
            } => Ok((fragments.clone(), *reason, *usage)),
            Turn::Fail(kind) => Err(ModelError::from_static(*kind, failure_message(*kind))),
            Turn::Slow { delay_ms, then } => {
                // The delay is a real sleep so the runtime can schedule a racing cancellation.
                tokio::time::sleep(std::time::Duration::from_millis(*delay_ms)).await;
                self.check_cancelled()?;
                Box::pin(self.resolve(then)).await
            }
        }
    }

    /// Builds the event sequence for a resolved turn.
    ///
    /// The sequence starts at zero and advances by one, which is the contract
    /// [`crate::stream::StreamValidator`] enforces. Building it in one place means the streaming and
    /// non-streaming paths describe the same turn identically.
    fn events(
        fragments: &[String],
        reason: FinishReason,
        usage: Option<TokenUsage>,
    ) -> Vec<StreamEnvelope> {
        let mut events = Vec::with_capacity(fragments.len() + 3);
        events.push(StreamEvent::Started);
        for fragment in fragments {
            events.push(StreamEvent::TextDelta {
                text: fragment.clone(),
            });
        }
        if let Some(usage) = usage {
            events.push(StreamEvent::Usage { usage });
        }
        events.push(StreamEvent::Finished { reason });
        events
            .into_iter()
            .enumerate()
            .map(|(index, event)| {
                // The bound is unreachable for a scripted turn, but `as u64` on a huge index would
                // silently wrap; a saturating conversion cannot.
                StreamEnvelope::new(u64::try_from(index).unwrap_or(u64::MAX), event)
            })
            .collect()
    }
}

/// A fixed, non-secret explanation for a scripted failure kind.
fn failure_message(kind: ModelErrorKind) -> &'static str {
    match kind {
        ModelErrorKind::Transient => "the scripted provider returned a transient failure",
        ModelErrorKind::ContentRefusal => "the scripted provider refused the output",
        ModelErrorKind::RateLimited => "the scripted provider rate limited the request",
        ModelErrorKind::Timeout => "the scripted provider timed out",
        ModelErrorKind::Cancelled => "the model call was cancelled",
        _ => "the scripted provider returned a failure",
    }
}

#[async_trait::async_trait]
impl ModelGateway for ScriptedModel {
    fn provider_id(&self) -> &ProviderId {
        &self.provider
    }

    async fn capabilities(&self, model: &ModelId) -> Result<ModelCapabilities, ModelError> {
        if model != &self.model {
            return Err(ModelError::from_static(
                ModelErrorKind::ModelNotFound,
                "the scripted model serves exactly one model id",
            ));
        }
        Ok(self.capabilities)
    }

    async fn complete(&self, request: ChatRequest) -> Result<ChatResponse, ModelError> {
        self.check_cancelled()?;
        self.record(&request);
        let turn = self.next_turn()?;
        let (fragments, reason, usage) = self.resolve(&turn).await?;

        let text = fragments.concat();
        Ok(ChatResponse::new(
            self.provider.clone(),
            request.model().clone(),
            vec![OutputContent::Text { text }],
            reason,
        )
        .with_usage(usage))
    }

    async fn stream(&self, request: ChatRequest) -> Result<ModelStream, ModelError> {
        self.check_cancelled()?;
        self.record(&request);
        let turn = self.next_turn()?;
        let (fragments, reason, usage) = self.resolve(&turn).await?;
        let events = Self::events(&fragments, reason, usage);
        // A scripted stream is fully realized before it is returned, which is what makes it
        // deterministic: there is no provider state that could deliver events in a different order.
        Ok(Box::pin(futures_util::stream::iter(
            events.into_iter().map(Ok),
        )))
    }

    async fn health(&self) -> ProviderHealth {
        // A scripted adapter is always reachable: it performs no I/O. Reporting `Degraded` for a
        // scripted failure would describe the *script* as a transport problem, and a health probe is
        // about reachability, not about what the next turn happens to say.
        ProviderHealth::new(
            ProviderStatus::Ready,
            "the scripted model requires no provider",
        )
    }
}

/// Convenience constructor used by tests and by the first end-to-end run.
///
/// # Errors
///
/// Returns [`ModelError`] under the same conditions as [`ScriptedModel::new`].
pub fn scripted(
    provider: &str,
    model: &str,
    turns: Vec<Turn>,
) -> Result<ScriptedModel, ModelError> {
    ScriptedModel::new(provider, model, turns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::ChatMessage;
    use crate::stream::{StreamEventError, StreamValidator};
    use futures_util::StreamExt as _;

    fn request() -> ChatRequest {
        ChatRequest::new(
            ModelId::new("scripted-small").unwrap_or_else(|error| panic!("model id: {error}")),
            vec![ChatMessage::user("hello")],
            jarvis_core::CorrelationId::new(),
        )
    }

    fn model(turns: Vec<Turn>) -> ScriptedModel {
        scripted(SCRIPTED_PROVIDER, "scripted-small", turns)
            .unwrap_or_else(|error| panic!("scripted model: {error}"))
    }

    #[tokio::test]
    async fn a_streamed_turn_produces_a_contiguous_sequence_ending_in_finished() {
        let model = model(vec![Turn::streamed(&["Hel", "lo", " world"])]);
        let mut stream = model
            .stream(request())
            .await
            .unwrap_or_else(|error| panic!("stream must open: {error}"));

        let mut validator = StreamValidator::new();
        let mut text = String::new();
        while let Some(item) = stream.next().await {
            let envelope =
                item.unwrap_or_else(|error| panic!("the scripted turn must not fail: {error}"));
            validator
                .accept(envelope.clone())
                .unwrap_or_else(|error| panic!("sequence must be valid: {error}"));
            if let StreamEvent::TextDelta { text: fragment } = envelope.event() {
                text.push_str(fragment);
            }
        }

        assert_eq!(text, "Hello world");
        validator
            .finish()
            .unwrap_or_else(|error| panic!("the stream must finish: {error}"));
        assert_eq!(model.calls(), 1, "one call is one provider attempt");
    }

    /// The sequence is the contract, so it is asserted directly rather than inferred from the text.
    #[tokio::test]
    async fn the_sequence_starts_at_zero_and_advances_by_one() {
        let model = model(vec![Turn::streamed(&["a", "b"])]);
        let mut stream = model
            .stream(request())
            .await
            .unwrap_or_else(|error| panic!("stream must open: {error}"));

        let mut sequences = Vec::new();
        while let Some(item) = stream.next().await {
            let envelope = item.unwrap_or_else(|error| panic!("no failure expected: {error}"));
            sequences.push(envelope.sequence());
        }
        assert_eq!(
            sequences,
            vec![0, 1, 2, 3, 4],
            "started, two deltas, usage, finished"
        );
    }

    /// A gap must be detectable, which is what the validator is for. Injecting one proves the
    /// validator is actually consulted rather than merely present.
    #[test]
    fn a_dropped_event_is_reported_as_a_gap_not_a_complete_answer() {
        let events = ScriptedModel::events(
            &["a".to_owned(), "b".to_owned()],
            FinishReason::Stop,
            Some(TokenUsage::new(1, 1)),
        );
        let mut validator = StreamValidator::new();
        // Skip the second delta, exactly as a lost network chunk would.
        for (index, envelope) in events.iter().enumerate() {
            if index == 2 {
                continue;
            }
            let result = validator.accept(envelope.clone());
            if index > 2 {
                assert_eq!(
                    result,
                    Err(StreamEventError::Gap),
                    "a skipped sequence must be reported as a gap"
                );
                return;
            }
        }
        panic!("the fixture must contain an event after the skipped one");
    }

    #[tokio::test]
    async fn a_transient_failure_is_a_normalized_error_with_a_retryable_category() {
        let model = model(vec![Turn::transient_failure()]);
        let Err(error) = model.complete(request()).await else {
            panic!("a scripted failure must not report success");
        };
        assert_eq!(error.kind(), ModelErrorKind::Transient);
        assert_eq!(
            error.kind().domain_code(),
            jarvis_core::ErrorCode::TransientUpstream
        );
    }

    /// The last turn repeats, so an extra call is a visible call count rather than an error that
    /// masks what the executor did.
    #[tokio::test]
    async fn the_final_turn_repeats_and_calls_are_counted() {
        let model = model(vec![Turn::answer("only turn")]);
        for _ in 0..3 {
            model
                .complete(request())
                .await
                .unwrap_or_else(|error| panic!("every call must be served: {error}"));
        }
        assert_eq!(model.calls(), 3);
    }

    /// A cancellation that arrives during a slow turn must surface as `Cancelled`, not as a timeout
    /// or a success. This is the behavior `A04` names.
    #[tokio::test]
    async fn a_cancellation_during_a_slow_turn_reports_cancellation() {
        let signal = std::sync::Arc::new(ScriptedCancellation::new());
        let model = model(vec![Turn::slow(10_000, Turn::answer("too late"))])
            .with_cancellation(std::sync::Arc::clone(&signal));

        let pending = model.complete(request());

        // Fire the signal from another task, so the cancellation genuinely races the sleep rather
        // than being set before the call begins.
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            signal.cancel();
        });
        let Err(error) = pending.await else {
            panic!("a cancelled turn must not succeed");
        };
        canceller
            .await
            .unwrap_or_else(|error| panic!("canceller: {error}"));

        assert_eq!(error.kind(), ModelErrorKind::Cancelled);
        assert_eq!(
            error.kind().domain_code(),
            jarvis_core::ErrorCode::Cancelled
        );
    }

    #[tokio::test]
    async fn an_unknown_model_id_is_not_found_rather_than_answered() {
        let model = model(vec![Turn::answer("x")]);
        let other =
            ModelId::new("different-model").unwrap_or_else(|error| panic!("model id: {error}"));
        let Err(error) = model.capabilities(&other).await else {
            panic!("a different model id must not be served");
        };
        assert_eq!(error.kind(), ModelErrorKind::ModelNotFound);
    }

    /// A script must have at least one turn. An empty script would answer nothing and could be
    /// mistaken for a model that produces no output.
    #[test]
    fn an_empty_script_is_refused() {
        assert!(
            scripted(SCRIPTED_PROVIDER, "scripted-small", Vec::new()).is_err(),
            "an empty script must be refused rather than answering nothing"
        );
    }

    /// The non-streaming and streaming paths must describe the same turn. A disagreement would make
    /// a test that used one path prove nothing about the other.
    #[tokio::test]
    async fn the_completed_answer_matches_the_streamed_answer() {
        let streamed_model = model(vec![Turn::streamed(&["ab", "cd"])]);
        let mut stream = streamed_model
            .stream(request())
            .await
            .unwrap_or_else(|error| panic!("stream must open: {error}"));
        let mut streamed = String::new();
        while let Some(item) = stream.next().await {
            let envelope = item.unwrap_or_else(|error| panic!("no failure expected: {error}"));
            if let StreamEvent::TextDelta { text } = envelope.event() {
                streamed.push_str(text);
            }
        }

        let complete_model = model(vec![Turn::streamed(&["ab", "cd"])]);
        let response = complete_model
            .complete(request())
            .await
            .unwrap_or_else(|error| panic!("complete must succeed: {error}"));

        assert_eq!(response.text(), "abcd");
        assert_eq!(streamed, response.text());
        assert_eq!(response.finish_reason(), FinishReason::Stop);
    }

    /// The adapter must be usable through the production port as a trait object, which is how the
    /// daemon will hold it. A type that only works concretely would not be.
    #[tokio::test]
    async fn the_adapter_is_usable_through_the_port() {
        let adapter: std::sync::Arc<dyn ModelGateway> =
            std::sync::Arc::new(model(vec![Turn::answer("through the port")]));
        assert_eq!(adapter.provider_id().as_str(), SCRIPTED_PROVIDER);
        assert_eq!(adapter.health().await.status(), ProviderStatus::Ready);
        let response = adapter
            .complete(request())
            .await
            .unwrap_or_else(|error| panic!("complete must succeed: {error}"));
        assert_eq!(response.text(), "through the port");
    }
}
