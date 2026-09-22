//! The executor port: what JARVIS calls to run one tool, and what an adapter must answer.
//!
//! # Why the port is here and not in `jarvis-core`
//!
//! The same reason `P3-003` gives for the policy engine (ADR-0017), one step further. A port is
//! usually core's to own, and `docs/architecture/repository-layout.md` does list "repository and
//! service ports expressed in JARVIS types" under `jarvis-core`. But an executor's **arguments** are
//! the tool's validated input and its **result** is the tool's output, and validating either needs a
//! JSON Schema implementation. Putting the port in core would move `jsonschema` into a crate whose own
//! rule is that it may not have a vendor SDK.
//!
//! So the port lives in the adapter crate, which is the same call `jarvis-models` makes for
//! `ModelGateway`: that port takes `jarvis_core` types only, and this one takes the canonical tool
//! types a JARVIS-owned contract defines rather than a provider's.
//!
//! # The adapter's inputs are all values
//!
//! [`ToolExecutionRequest`] carries a validated argument object, an [`AuthorizationReceipt`], an
//! idempotency key, a deadline, and a correlation identity. It carries no store handle, no workspace
//! object, no secret resolver, and no cancellation token, because
//! `docs/architecture/tools-and-connectors.md` requires that the adapter "does not receive an
//! unrestricted application context" — and the way to make that true is for the request type to have
//! no field capable of holding one.
//!
//! **A secret resolver and a cancellation token are genuinely absent, not forgotten.** Both belong to
//! `P3-008`/`P3-011`: a connector's token lifecycle is the connector's, and cancellation needs a run
//! to signal. Stating that here is deliberate, because an adapter author reading only this file would
//! otherwise assume they were omitted by mistake and reach for a global.

use std::fmt;

use async_trait::async_trait;
use jarvis_core::{CorrelationId, UtcTimestamp};
use serde_json::Value;

use crate::execution::{AuthorizationReceipt, IdempotencyKey, ToolCallResult};
use crate::identifier::ToolId;

/// Explains why an execution request was rejected.
#[derive(Clone, Copy, Debug, Eq, thiserror::Error, PartialEq)]
pub enum ExecutionRequestError {
    /// The arguments were not a JSON object.
    ///
    /// A validated tool call's input schema is an object, so non-object arguments did not come from a
    /// validated call. Refused here rather than at the adapter, so every adapter does not have to
    /// restate the rule.
    #[error("tool arguments must be a JSON object")]
    ArgumentsNotAnObject,
    /// The request cited an authority that had already lapsed.
    #[error("the authorization receipt had expired when the request was built")]
    ReceiptExpired,
}

/// One validated call to one tool.
///
/// Borrows nothing and owns everything, so the request can be moved to a worker without carrying a
/// reference into JARVIS state.
#[derive(Clone, Debug)]
pub struct ToolExecutionRequest {
    call_id: String,
    tool: ToolId,
    tool_version: String,
    arguments: Value,
    receipt: AuthorizationReceipt,
    idempotency_key: IdempotencyKey,
    deadline: UtcTimestamp,
    correlation_id: CorrelationId,
}

/// The declared fields of an execution request.
#[derive(Clone, Debug)]
pub struct ToolExecutionRequestParts {
    /// The durable call identifier.
    pub call_id: String,
    /// The tool to run.
    pub tool: ToolId,
    /// The tool version the arguments were validated against.
    pub tool_version: String,
    /// The validated argument object.
    pub arguments: Value,
    /// The authority for this call.
    pub receipt: AuthorizationReceipt,
    /// The key a provider deduplicates on.
    pub idempotency_key: IdempotencyKey,
    /// When the call must stop.
    pub deadline: UtcTimestamp,
    /// The correlation identity shared with the originating request.
    pub correlation_id: CorrelationId,
}

impl ToolExecutionRequest {
    /// Validates and builds a request.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutionRequestError::ArgumentsNotAnObject`] when the arguments are not a JSON
    /// object, or [`ExecutionRequestError::ReceiptExpired`] when the cited authority had lapsed by the
    /// request's own timestamp.
    ///
    /// # Why the receipt is re-checked here
    ///
    /// The policy engine already refused a lapsed approval (`P3-003`), and `P3-004` refuses a decision
    /// after the expiry. This third check is not redundancy for its own sake: it closes the window
    /// between the decision and the call, which can contain a queue wait. A request built with an
    /// authority that lapsed while it waited must not reach an adapter, and the request's own timestamp
    /// is what makes the check deterministic rather than dependent on when it happens to run.
    pub fn new(parts: ToolExecutionRequestParts) -> Result<Self, ExecutionRequestError> {
        let ToolExecutionRequestParts {
            call_id,
            tool,
            tool_version,
            arguments,
            receipt,
            idempotency_key,
            deadline,
            correlation_id,
        } = parts;

        if !arguments.is_object() {
            return Err(ExecutionRequestError::ArgumentsNotAnObject);
        }
        if !receipt.is_valid_at(receipt.issued_at()) {
            return Err(ExecutionRequestError::ReceiptExpired);
        }

        Ok(Self {
            call_id,
            tool,
            tool_version,
            arguments,
            receipt,
            idempotency_key,
            deadline,
            correlation_id,
        })
    }

    /// Returns the durable call identifier.
    #[must_use]
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    /// Returns the tool to run.
    #[must_use]
    pub const fn tool(&self) -> &ToolId {
        &self.tool
    }

    /// Returns the tool version the arguments were validated against.
    #[must_use]
    pub fn tool_version(&self) -> &str {
        &self.tool_version
    }

    /// Returns the validated argument object.
    #[must_use]
    pub const fn arguments(&self) -> &Value {
        &self.arguments
    }

    /// Returns the authority for this call.
    #[must_use]
    pub const fn receipt(&self) -> &AuthorizationReceipt {
        &self.receipt
    }

    /// Returns the key a provider deduplicates on.
    #[must_use]
    pub const fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    /// Returns when the call must stop.
    #[must_use]
    pub const fn deadline(&self) -> UtcTimestamp {
        self.deadline
    }

    /// Returns the correlation identity.
    #[must_use]
    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    /// Returns whether the deadline had passed at an instant.
    ///
    /// Offered rather than enforced so an adapter can check it at a point of its own choosing — before
    /// a network write, for example — while the pipeline decides what a missed deadline becomes.
    #[must_use]
    pub fn is_past_deadline(&self, now: UtcTimestamp) -> bool {
        now.unix_nanos() >= self.deadline.unix_nanos()
    }
}

/// Explains why an adapter could not produce a result.
///
/// Deliberately **not** a mirror of [`crate::ToolOutcome`]. This is the adapter saying "I could not
/// run", and the outcome is the adapter saying "here is what happened when I did". An adapter that
/// returns an error has not reported an effect at all, which is why the honest outcomes
/// (`Unknown`, `Failed`) are reachable only through a successful return.
#[derive(Clone, Debug, Eq, thiserror::Error, PartialEq)]
pub enum AdapterError {
    /// The tool is not implemented by this adapter.
    ///
    /// A configuration fault rather than a runtime one: the registry offered a tool no adapter can run.
    #[error("this adapter does not implement {tool}")]
    NotImplemented {
        /// The tool that could not be run.
        tool: String,
    },
    /// The adapter refused before reaching a provider.
    ///
    /// The request never left, so nothing happened — a stronger statement than `Failed`, and the one
    /// an adapter must use only when it is certain.
    #[error("the adapter refused the call before reaching a provider: {reason}")]
    RefusedBeforeReaching {
        /// A bounded explanation.
        reason: String,
    },
    /// The provider was reached and its answer could not be obtained.
    ///
    /// **The ambiguous case, and the reason this variant exists separately.** An adapter that knows it
    /// sent a request and does not know the answer must return this rather than
    /// [`Self::RefusedBeforeReaching`], because a caller that saw a plain error would retry — and for a
    /// non-idempotent effect the retry is a second effect.
    #[error("the provider was reached but the outcome is unknown: {reason}")]
    AmbiguousAfterReaching {
        /// A bounded explanation.
        reason: String,
    },
    /// The provider answered with a failure the adapter is certain produced no effect.
    #[error("the provider refused the call: {reason}")]
    ProviderRefused {
        /// A bounded explanation.
        reason: String,
    },
}

/// Runs one tool.
///
/// An implementation is an adapter: it talks to one provider, one runtime, or one local primitive. It
/// does not decide anything — policy and approval already happened — and it does not persist anything,
/// because the audit record is the pipeline's (`P3-005` in `apps/jarvisd`).
///
/// # What an implementation must not do
///
/// - It must not retry. Retry is a declaration on the tool contract
///   ([`crate::ToolDefinition::may_retry_automatically`]) and a decision of the pipeline, and an
///   adapter that retried internally would make a non-idempotent effect repeat without the decision
///   being visible or recorded.
/// - It must not interpret the receipt's expiry as permission. The pipeline revalidates before calling;
///   an adapter that extended authority on its own would defeat the lapse.
/// - It must not turn a success-shaped response into [`crate::ToolOutcome::Confirmed`] without
///   evidence. `docs/architecture/tools-and-connectors.md`: "Do not turn a success-sounding string
///   into proof."
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Returns a stable identifier for diagnostics.
    ///
    /// `&'static str` rather than an elided `&str`: an adapter's name is a compile-time literal, not
    /// a value borrowed from the request, and saying so removes a lint that fires on every
    /// implementation that returns a literal.
    fn adapter_id(&self) -> &'static str;

    /// Runs one call.
    ///
    /// Returns [`ToolCallResult`] for any outcome the adapter could establish, including
    /// [`crate::ToolOutcome::Unknown`], and [`AdapterError`] only when it could not establish an
    /// outcome at all. The distinction is the point of the return type: an adapter that reaches a
    /// provider always has something to report, and the one thing it must not do is report `Failed`
    /// when it does not know.
    ///
    /// # Errors
    ///
    /// Returns [`AdapterError`] when the adapter cannot run the tool, refused before reaching a
    /// provider, or reached one without learning the outcome.
    async fn execute(&self, request: &ToolExecutionRequest)
    -> Result<ToolCallResult, AdapterError>;
}

impl fmt::Debug for dyn ToolExecutor {
    /// Names the adapter without printing its internals.
    ///
    /// An implementation may hold a provider client or a credential-bearing request object, neither of
    /// which belongs in a formatted value.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolExecutor")
            .field("adapter_id", &self.adapter_id())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::{AuthorizationReceiptParts, ProviderEvidence, ToolCallResult};
    use crate::{ToolOutcome, ToolOutcomeRecord};
    use serde_json::json;

    fn at(offset_seconds: i128) -> UtcTimestamp {
        UtcTimestamp::from_unix_nanos(1_774_000_000_000_000_000 + offset_seconds * 1_000_000_000)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// The digest of a tool, version, and the argument set the request fixtures use.
    ///
    /// Computed rather than a placeholder string, because `AuthorizationReceipt::new` now verifies the
    /// digest against the arguments it is given. A fixture has to satisfy the check it exists for.
    fn digest(tool: &str, version: &str) -> jarvis_core::CanonicalIntentHash {
        jarvis_core::CanonicalIntentHash::compute(tool, version, &arguments())
            .unwrap_or_else(|error| panic!("{error}"))
    }

    /// The arguments the receipt fixtures cover, so digest and request agree.
    fn arguments() -> Value {
        json!({"to": "a@example.invalid"})
    }

    /// An allowing decision, produced by `evaluate` rather than fabricated.
    ///
    /// `PolicyDecision` has a private constructor precisely so a decision must come from an
    /// evaluation; a fixture that could build one directly could fabricate authority.
    fn allowing() -> crate::PolicyDecision {
        use crate::evaluation::{
            ActorAuthority, AuthenticationStrength, PolicyRequest, TargetAssessment,
            WorkspacePolicy, evaluate,
        };
        use crate::policy::{ApprovalPolicy, Availability, Idempotency, RetryDeclaration};
        use crate::risk::Risk;
        use crate::scope::ScopeSet;
        use crate::{EffectSet, ToolEffect, ToolSource};
        use jarvis_core::SessionChannel;

        let definition = crate::ToolDefinition::new(crate::ToolDefinitionParts {
            id: ToolId::new("jarvis.mail.send").unwrap_or_else(|error| panic!("{error}")),
            version: "1.0.0".to_owned(),
            title: "Send mail".to_owned(),
            description: "Sends one message.".to_owned(),
            input_schema: crate::ToolSchema::from_value(json!({
                "$schema": crate::TOOL_SCHEMA_DIALECT,
                "type": "object"
            }))
            .unwrap_or_else(|error| panic!("{error}")),
            output_schema: crate::ToolSchema::from_value(json!({
                "$schema": crate::TOOL_SCHEMA_DIALECT,
                "type": "object"
            }))
            .unwrap_or_else(|error| panic!("{error}")),
            effects: EffectSet::single(ToolEffect::ReadOnly),
            risk: 0,
            required_scopes: ScopeSet::none(),
            approval: ApprovalPolicy::Auto,
            timeout_seconds: 30,
            retry: RetryDeclaration::none(),
            idempotency: Idempotency::Required,
            source: ToolSource::Native,
            availability: Availability::Available,
            sensitivity: crate::ToolSensitivity::new(
                jarvis_core::Sensitivity::Internal,
                jarvis_core::Sensitivity::Internal,
            ),
        })
        .unwrap_or_else(|error| panic!("{error}"));

        let _ = Risk::Minimal;
        let decision = evaluate(&PolicyRequest {
            definition: &definition,
            actor: ActorAuthority::active(ScopeSet::none()),
            workspace: &WorkspacePolicy::default(),
            channel: SessionChannel::Cli,
            claimed_strength: AuthenticationStrength::Present,
            available: true,
            target: TargetAssessment::none(),
        });
        assert!(
            decision.is_allowed(),
            "the fixture needs an allowing decision, got {:?}",
            decision.reason_code()
        );
        decision
    }

    fn receipt(expires_at: Option<UtcTimestamp>) -> AuthorizationReceipt {
        let approval = expires_at.map(|expires_at| crate::execution::ApprovalCitation {
            approval_id: "0198f000-0000-7000-8000-0000000000e2".to_owned(),
            approver_id: "user-2".to_owned(),
            approved_at: at(0),
            expires_at,
        });
        AuthorizationReceipt::new(AuthorizationReceiptParts {
            receipt_id: "0198f000-0000-7000-8000-0000000000e1".to_owned(),
            tool: ToolId::new("jarvis.mail.send").unwrap_or_else(|error| panic!("{error}")),
            tool_version: "1.0.0".to_owned(),
            arguments: arguments(),
            intent_hash: digest("jarvis.mail.send", "1.0.0"),
            policy_version: "policy-3".to_owned(),
            decision: allowing(),
            approval,
            correlation_id: CorrelationId::new(),
            issued_at: at(0),
        })
        .unwrap_or_else(|error| panic!("{error}"))
    }

    fn parts(receipt: AuthorizationReceipt, arguments: Value) -> ToolExecutionRequestParts {
        ToolExecutionRequestParts {
            call_id: "0198f000-0000-7000-8000-0000000000e3".to_owned(),
            tool: ToolId::new("jarvis.mail.send").unwrap_or_else(|error| panic!("{error}")),
            tool_version: "1.0.0".to_owned(),
            arguments,
            receipt,
            idempotency_key: IdempotencyKey::generate().unwrap_or_else(|error| panic!("{error}")),
            deadline: at(30),
            correlation_id: CorrelationId::new(),
        }
    }

    /// **A request carries only values, and every field a caller needs is present.**
    ///
    /// The structural claim behind "the adapter does not receive an unrestricted application context":
    /// there is no field capable of holding a handle.
    #[test]
    fn a_request_carries_values_only() {
        let request = ToolExecutionRequest::new(parts(
            receipt(None),
            json!({"to": "a@example.invalid", "subject": "hi"}),
        ))
        .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(request.call_id(), "0198f000-0000-7000-8000-0000000000e3");
        assert_eq!(request.tool().to_string(), "jarvis.mail.send");
        assert_eq!(request.tool_version(), "1.0.0");
        assert_eq!(request.arguments()["to"], "a@example.invalid");
        assert_eq!(request.idempotency_key().as_str().len(), 32);
        assert_eq!(request.deadline(), at(30));
        assert_eq!(request.receipt().tool(), "jarvis.mail.send");
        assert!(!request.is_past_deadline(at(29)));
        assert!(request.is_past_deadline(at(30)));
    }

    /// Non-object arguments are refused, because a validated call always has an object.
    #[test]
    fn non_object_arguments_are_refused() {
        for arguments in [json!([1, 2]), json!("text"), json!(5), json!(null)] {
            assert_eq!(
                ToolExecutionRequest::new(parts(receipt(None), arguments)).err(),
                Some(ExecutionRequestError::ArgumentsNotAnObject)
            );
        }
    }

    /// **A request citing a lapsed authority is refused.**
    ///
    /// The window this closes is the queue wait between the decision and the call: an approval that was
    /// valid when policy allowed it can lapse before an adapter would run.
    #[test]
    fn a_lapsed_receipt_is_refused() {
        // The receipt was issued at second 0 and its approval lapses at second 5. Building the request
        // at second 10 means the authority lapsed.
        let mut request_parts = parts(receipt(Some(at(5))), json!({"to": "a@example.invalid"}));
        request_parts.receipt = {
            // The constructor stamps `issued_at` from the parts, so move both to model a late request.
            AuthorizationReceipt::new(AuthorizationReceiptParts {
                receipt_id: "0198f000-0000-7000-8000-0000000000e1".to_owned(),
                tool: ToolId::new("jarvis.mail.send").unwrap_or_else(|error| panic!("{error}")),
                tool_version: "1.0.0".to_owned(),
                arguments: arguments(),
                intent_hash: digest("jarvis.mail.send", "1.0.0"),
                policy_version: "policy-3".to_owned(),
                decision: allowing(),
                approval: Some(crate::execution::ApprovalCitation {
                    approval_id: "0198f000-0000-7000-8000-0000000000e2".to_owned(),
                    approver_id: "user-2".to_owned(),
                    approved_at: at(0),
                    expires_at: at(5),
                }),
                correlation_id: CorrelationId::new(),
                issued_at: at(10),
            })
            .unwrap_or_else(|error| panic!("{error}"))
        };
        assert_eq!(
            ToolExecutionRequest::new(request_parts).err(),
            Some(ExecutionRequestError::ReceiptExpired)
        );

        // The same authority inside its lifetime is accepted.
        let mut inside = parts(receipt(Some(at(20))), json!({"to": "a@example.invalid"}));
        inside.receipt = AuthorizationReceipt::new(AuthorizationReceiptParts {
            receipt_id: "0198f000-0000-7000-8000-0000000000e1".to_owned(),
            tool: ToolId::new("jarvis.mail.send").unwrap_or_else(|error| panic!("{error}")),
            tool_version: "1.0.0".to_owned(),
            arguments: arguments(),
            intent_hash: digest("jarvis.mail.send", "1.0.0"),
            policy_version: "policy-3".to_owned(),
            decision: allowing(),
            approval: Some(crate::execution::ApprovalCitation {
                approval_id: "0198f000-0000-7000-8000-0000000000e2".to_owned(),
                approver_id: "user-2".to_owned(),
                approved_at: at(0),
                expires_at: at(20),
            }),
            correlation_id: CorrelationId::new(),
            issued_at: at(10),
        })
        .unwrap_or_else(|error| panic!("{error}"));
        assert!(ToolExecutionRequest::new(inside).is_ok());
    }

    /// An empty argument object is valid, because a parameterless tool has one.
    #[test]
    fn an_empty_argument_object_is_valid() {
        let request = ToolExecutionRequest::new(parts(receipt(None), json!({})))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            request
                .arguments()
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
        );
    }

    /// A test double that reports a chosen outcome, so the port's shape is exercised.
    struct ScriptedExecutor {
        outcome: ToolOutcome,
        error: Option<AdapterError>,
    }

    #[async_trait]
    impl ToolExecutor for ScriptedExecutor {
        fn adapter_id(&self) -> &'static str {
            "scripted"
        }

        async fn execute(
            &self,
            request: &ToolExecutionRequest,
        ) -> Result<ToolCallResult, AdapterError> {
            if let Some(error) = self.error.clone() {
                return Err(error);
            }
            let record = match self.outcome {
                ToolOutcome::Confirmed => ToolOutcomeRecord::confirmed("provider-id-1"),
                ToolOutcome::Failed => ToolOutcomeRecord::failed("the provider refused"),
                other => ToolOutcomeRecord::new(other),
            }
            .unwrap_or_else(|error| panic!("{error}"));
            Ok(ToolCallResult::new(
                record,
                ProviderEvidence::new("provider-id-1").ok(),
                None,
                request.deadline(),
            ))
        }
    }

    /// The port is usable as a trait object, which is how the pipeline will hold it.
    #[tokio::test]
    async fn the_port_is_object_safe_and_reports_an_outcome() {
        let executor: Box<dyn ToolExecutor> = Box::new(ScriptedExecutor {
            outcome: ToolOutcome::Confirmed,
            error: None,
        });
        assert_eq!(executor.adapter_id(), "scripted");
        let request = ToolExecutionRequest::new(parts(receipt(None), json!({})))
            .unwrap_or_else(|error| panic!("{error}"));
        let result = executor
            .execute(&request)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(result.outcome(), ToolOutcome::Confirmed);
        assert_eq!(
            result.evidence().map(ProviderEvidence::as_str),
            Some("provider-id-1")
        );

        // `Debug` names the adapter and does not print a body.
        let rendered = format!("{executor:?}");
        assert!(rendered.contains("scripted"));
    }

    /// **An ambiguous adapter error is distinguishable from a definite refusal.**
    ///
    /// The distinction a caller must not lose: the first forbids a blind retry and the second permits
    /// one, and an adapter that reported both as one error would make the difference unknowable.
    #[tokio::test]
    async fn an_ambiguous_error_is_distinguishable_from_a_definite_one() {
        for (error, expected_ambiguous) in [
            (
                AdapterError::AmbiguousAfterReaching {
                    reason: "the connection dropped after the request was written".to_owned(),
                },
                true,
            ),
            (
                AdapterError::RefusedBeforeReaching {
                    reason: "no credential is configured".to_owned(),
                },
                false,
            ),
            (
                AdapterError::ProviderRefused {
                    reason: "the provider returned 403".to_owned(),
                },
                false,
            ),
            (
                AdapterError::NotImplemented {
                    tool: "jarvis.mail.send".to_owned(),
                },
                false,
            ),
        ] {
            let executor = ScriptedExecutor {
                outcome: ToolOutcome::Unknown,
                error: Some(error.clone()),
            };
            let request = ToolExecutionRequest::new(parts(receipt(None), json!({})))
                .unwrap_or_else(|error| panic!("{error}"));
            let Err(returned) = executor.execute(&request).await else {
                panic!("a configured error must be returned");
            };
            assert_eq!(returned, error);
            assert_eq!(
                matches!(returned, AdapterError::AmbiguousAfterReaching { .. }),
                expected_ambiguous,
                "{returned} classification"
            );
        }
    }

    /// A submitted outcome is reported through the port and is not terminal.
    #[tokio::test]
    async fn a_submitted_outcome_is_reported_and_is_not_terminal() {
        let executor = ScriptedExecutor {
            outcome: ToolOutcome::Submitted,
            error: None,
        };
        let request = ToolExecutionRequest::new(parts(receipt(None), json!({})))
            .unwrap_or_else(|error| panic!("{error}"));
        let result = executor
            .execute(&request)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(result.outcome(), ToolOutcome::Submitted);
        assert!(!result.is_terminal());
    }
}
