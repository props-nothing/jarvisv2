//! Tool call execution: the bounded invocation record, its idempotency key, and its receipt.
//!
//! # The four things this module keeps separate, and why
//!
//! `docs/architecture/tools-and-connectors.md` states two rules that decide the shape of every type
//! here, and both are about **not merging two values that look similar**:
//!
//! 1. *"Do not turn a success-sounding string into proof. Preserve provider IDs and receipts
//!    separately from user-facing text."* So a provider's receipt is [`ProviderEvidence`] and what a
//!    model or user reads is [`BoundedOutput`]. They are different fields with different bounds and
//!    different visibility, because a provider that returns `{"status":"ok"}` is a provider that said
//!    something, not one that proved anything.
//! 2. *"The executable adapter receives an authorization receipt, secret resolver, cancellation
//!    token, deadline, idempotency key, and correlation IDs. It does not receive an unrestricted
//!    application context."* So [`AuthorizationReceipt`] is a closed set of facts about **this** call
//!    and carries no store handle, no workspace object, and no secret resolver — the adapter's inputs
//!    are all values, which makes "the adapter reached into JARVIS state" unrepresentable rather than
//!    forbidden.
//!
//! # Why the idempotency key is not the intent hash
//!
//! They answer different questions and coincide only by accident.
//!
//! - The **intent hash** (`P3-004`) is what an approval binds to. It is deterministic: the same
//!   action hashes the same, which is what lets a stored approval be revalidated against a recomputed
//!   digest.
//! - The **idempotency key** is what a provider deduplicates on. It must be *unique per logical
//!   call*, so a deliberate second call that happens to have identical arguments is a second call.
//!
//! Using the intent hash as the key would make "archive this folder, then archive it again after a
//! new message arrived" silently collapse into one operation — the second call would be deduplicated
//! by the provider as a repeat of the first. So the key is **generated once when the call is admitted
//! and persisted**, and it is the persisted value that is forwarded.
//!
//! # Outcome honesty is the adapter's obligation, and it is stated rather than assumed
//!
//! [`ToolCallResult`] carries a [`ToolOutcome`] the adapter reports. The adapter is the only party
//! that can distinguish "I never sent it" from "I sent it and lost the answer", so it must choose
//! between [`ToolOutcome::Failed`] and [`ToolOutcome::Unknown`] — and the port's documentation says
//! which choice is the honest one when it cannot tell. Nothing here infers an outcome from an error,
//! because an inference would be exactly the "success-sounding string into proof" step in reverse.

use std::fmt;

use jarvis_core::{CorrelationId, UtcTimestamp};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::identifier::ToolId;
use crate::risk::Risk;
use crate::{ToolOutcome, ToolOutcomeRecord};

/// Bytes of tool output that may be retained.
///
/// Bounded because tool output is untrusted external content that reaches a model. `P3-006`'s
/// filesystem tool and every connector can produce arbitrarily much, so without a bound one call can
/// consume a run's whole context budget — which is a denial of service on the model path, not merely
/// a large row.
///
/// 32 KiB is deliberately larger than [`crate::MAX_TOOL_SCHEMA_BYTES`]'s half: output is the thing a
/// run actually needs, and truncating it too eagerly would make a working tool look broken. The
/// truncation is *reported* rather than silent, which is what makes a generous bound safe.
pub const MAX_TOOL_OUTPUT_BYTES: usize = 32 * 1024;

/// Characters of provider evidence that may be retained.
///
/// Much smaller than the output bound, and for a different reason: evidence is a provider identifier,
/// a receipt, or a request id — a locator, never content. A longer value than this is not evidence but
/// a payload, and accepting it would put provider payload text into a durable record.
pub const MAX_PROVIDER_EVIDENCE_CHARS: usize = 256;

/// Characters in a policy version string.
pub const MAX_POLICY_VERSION_CHARS: usize = 64;

/// Explains why a bounded output was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum OutputError {
    /// The content exceeded the bound.
    ///
    /// Returned only by the strict constructor. Callers that accept truncation use
    /// [`BoundedOutput::truncating`], which cannot fail.
    #[error("tool output exceeds {MAX_TOOL_OUTPUT_BYTES} bytes")]
    TooLarge,
}

/// Explains why provider evidence was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum EvidenceError {
    /// The evidence was empty or oversized.
    #[error("provider evidence must be 1 to {MAX_PROVIDER_EVIDENCE_CHARS} characters")]
    Unusable,
}

/// Explains why an idempotency key was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum IdempotencyKeyError {
    /// The text was not the fixed encoded length.
    #[error("an idempotency key must be 32 lowercase hexadecimal characters")]
    Length,
    /// The text contained a non-hexadecimal character.
    #[error("an idempotency key contains a non-hexadecimal character")]
    NotHexadecimal,
    /// The platform random source was unavailable.
    #[error("random bytes are unavailable")]
    RandomUnavailable,
}

/// Explains why an authorization receipt was rejected.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ReceiptError {
    /// The policy version was empty or oversized.
    #[error("a policy version must be 1 to {MAX_POLICY_VERSION_CHARS} characters")]
    PolicyVersion,
    /// The receipt declared an approval but no approver.
    #[error("a receipt citing an approval must carry the approver and the decision time")]
    ApprovalIncomplete,
    /// The receipt declared an approver but no approval.
    #[error("a receipt naming an approver must cite an approval")]
    ApproverWithoutApproval,
}

/// Tool output, bounded and honestly marked when it was cut.
///
/// The truncation is a **flag rather than a suffix in the text**, because a marker in the content
/// could be authored by the tool and a reader could not tell a real marker from a truncated one. The
/// text is also cut on a character boundary, so a multi-byte character is never split.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BoundedOutput {
    content: String,
    truncated: bool,
}

impl BoundedOutput {
    /// Bounds the content, refusing rather than truncating when it is too large.
    ///
    /// # Errors
    ///
    /// Returns [`OutputError::TooLarge`] when the content exceeds [`MAX_TOOL_OUTPUT_BYTES`]. Provided
    /// for a caller that would rather fail than silently deliver a partial result — which is the right
    /// choice when the tool's output schema describes the *whole* result.
    pub fn strict(content: impl Into<String>) -> Result<Self, OutputError> {
        let content = content.into();
        if content.len() > MAX_TOOL_OUTPUT_BYTES {
            return Err(OutputError::TooLarge);
        }
        Ok(Self {
            content,
            truncated: false,
        })
    }

    /// Bounds the content, truncating on a character boundary and marking it.
    ///
    /// Cannot fail, because truncation is the fallback. The marker is a flag rather than text so that
    /// a tool's own content cannot be mistaken for it.
    #[must_use]
    pub fn truncating(content: impl Into<String>) -> Self {
        let content = content.into();
        if content.len() <= MAX_TOOL_OUTPUT_BYTES {
            return Self {
                content,
                truncated: false,
            };
        }
        // Cut on a character boundary: indexing a `String` at a byte offset panics if the offset is
        // inside a multi-byte character, so the largest valid boundary at or below the bound is used.
        let mut end = MAX_TOOL_OUTPUT_BYTES;
        while end > 0 && !content.is_char_boundary(end) {
            end -= 1;
        }
        Self {
            content: content[..end].to_owned(),
            truncated: true,
        }
    }

    /// Returns the bounded content.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Returns whether the content was cut.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Returns the retained byte count.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.content.len()
    }
}

impl fmt::Display for BoundedOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.content)?;
        if self.truncated {
            formatter.write_str("\n… [output truncated]")?;
        }
        Ok(())
    }
}

/// A provider's locator for one effect: an identifier, a receipt, or a request id.
///
/// Bounded tightly and deliberately not called "result": it is what makes an effect *checkable* later,
/// which is the property `P3-001`'s `ToolOutcome::Confirmed` requires. A confirmation with no
/// evidence is refused there; this is the type of the evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderEvidence(String);

impl ProviderEvidence {
    /// Validates a provider's evidence.
    ///
    /// # Errors
    ///
    /// Returns [`EvidenceError::Unusable`] when the value is empty, whitespace-only, or oversized. An
    /// empty evidence is not evidence, and accepting it would make `Confirmed` reachable with nothing
    /// behind it — which is the case `P3-001` refuses.
    pub fn new(value: impl Into<String>) -> Result<Self, EvidenceError> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_PROVIDER_EVIDENCE_CHARS {
            return Err(EvidenceError::Unusable);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Returns the evidence value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderEvidence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The key a provider deduplicates on, generated once per logical call.
///
/// 16 random bytes as 32 hexadecimal characters. Shorter than `P3-004`'s nonce because the threat is
/// different: a nonce is a secret an attacker would try to forge, while this is an identifier a
/// provider echoes back. 128 bits is far beyond collision range for any plausible number of calls, and
/// the value is not secret — it is forwarded to a provider by design.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Generates a key from the platform random source.
    ///
    /// # Errors
    ///
    /// Returns [`IdempotencyKeyError::RandomUnavailable`] when the platform source fails. Propagated
    /// rather than replaced: an idempotency key that could collide would make a provider deduplicate
    /// two unrelated calls, which is worse than refusing the call.
    pub fn generate() -> Result<Self, IdempotencyKeyError> {
        const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| IdempotencyKeyError::RandomUnavailable)?;
        let mut text = String::with_capacity(32);
        for byte in bytes {
            text.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
            text.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
        }
        Ok(Self(text))
    }

    /// Parses a key from hexadecimal text, normalizing case and trimming.
    ///
    /// # Errors
    ///
    /// Returns [`IdempotencyKeyError::Length`] or [`IdempotencyKeyError::NotHexadecimal`].
    pub fn parse(text: &str) -> Result<Self, IdempotencyKeyError> {
        let trimmed = text.trim();
        if trimmed.len() != 32 {
            return Err(IdempotencyKeyError::Length);
        }
        if !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(IdempotencyKeyError::NotHexadecimal);
        }
        Ok(Self(trimmed.to_ascii_lowercase()))
    }

    /// Returns the key as stored and forwarded.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for IdempotencyKey {
    type Error = IdempotencyKeyError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<IdempotencyKey> for String {
    fn from(key: IdempotencyKey) -> Self {
        key.0
    }
}

/// What an adapter is told about the authority under which it acts.
///
/// Every field is a **value**, not a handle. `docs/architecture/tools-and-connectors.md` requires that
/// the adapter "does not receive an unrestricted application context", and the way to guarantee that
/// is for the receipt to have no field capable of holding one — no store, no workspace object, no
/// secret resolver, no database pool.
///
/// # Why the approval fields are optional and grouped
///
/// A call that policy allowed directly has no approval, and a call that was held has one. The four
/// approval fields are therefore either all present or all absent, which the constructor enforces:
/// a receipt citing an approval but naming no approver would be an authorization nobody can be held
/// to, and one naming an approver without an approval would attribute a decision to a record that
/// does not exist.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AuthorizationReceipt {
    receipt_id: String,
    tool: String,
    tool_version: String,
    intent_hash: String,
    policy_version: String,
    risk_level: u8,
    approval_id: Option<String>,
    approver_id: Option<String>,
    approved_at: Option<UtcTimestamp>,
    expires_at: Option<UtcTimestamp>,
    correlation_id: CorrelationId,
    issued_at: UtcTimestamp,
}

/// The declared fields of an authorization receipt.
#[derive(Clone, Debug)]
pub struct AuthorizationReceiptParts {
    /// The receipt's own identifier, for the audit record.
    pub receipt_id: String,
    /// The tool's canonical identifier.
    pub tool: ToolId,
    /// The tool version the intent was built against.
    pub tool_version: String,
    /// The canonical intent digest the authority covers.
    pub intent_hash: String,
    /// The policy version that produced the decision.
    pub policy_version: String,
    /// The risk the decision was taken at.
    pub risk_level: Risk,
    /// The approval the authorization came from, when one was required.
    pub approval: Option<ApprovalCitation>,
    /// The correlation identity shared with the originating request.
    pub correlation_id: CorrelationId,
    /// When the receipt was issued.
    pub issued_at: UtcTimestamp,
}

/// The approval one authorization came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovalCitation {
    /// The approval request identifier.
    pub approval_id: String,
    /// The identity that decided it.
    pub approver_id: String,
    /// When it was decided.
    pub approved_at: UtcTimestamp,
    /// When it lapses.
    pub expires_at: UtcTimestamp,
}

impl AuthorizationReceipt {
    /// Builds and validates a receipt.
    ///
    /// # Errors
    ///
    /// Returns [`ReceiptError::PolicyVersion`] for an empty or oversized policy version.
    pub fn new(parts: AuthorizationReceiptParts) -> Result<Self, ReceiptError> {
        let AuthorizationReceiptParts {
            receipt_id,
            tool,
            tool_version,
            intent_hash,
            policy_version,
            risk_level,
            approval,
            correlation_id,
            issued_at,
        } = parts;

        let policy_version = policy_version.trim().to_owned();
        if policy_version.is_empty() || policy_version.chars().count() > MAX_POLICY_VERSION_CHARS {
            return Err(ReceiptError::PolicyVersion);
        }

        // The four approval fields move together. Constructing them from one `Option` is what makes a
        // half-cited approval unrepresentable rather than merely unlikely.
        let (approval_id, approver_id, approved_at, expires_at) = match approval {
            Some(citation) => {
                if citation.approver_id.trim().is_empty() {
                    return Err(ReceiptError::ApprovalIncomplete);
                }
                (
                    Some(citation.approval_id),
                    Some(citation.approver_id),
                    Some(citation.approved_at),
                    Some(citation.expires_at),
                )
            }
            None => (None, None, None, None),
        };

        Ok(Self {
            receipt_id,
            tool: tool.to_string(),
            tool_version,
            intent_hash,
            policy_version,
            risk_level: risk_level.level(),
            approval_id,
            approver_id,
            approved_at,
            expires_at,
            correlation_id,
            issued_at,
        })
    }

    /// Returns the receipt identifier.
    #[must_use]
    pub fn receipt_id(&self) -> &str {
        &self.receipt_id
    }

    /// Returns the tool the receipt authorizes.
    #[must_use]
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// Returns the tool version the intent was built against.
    #[must_use]
    pub fn tool_version(&self) -> &str {
        &self.tool_version
    }

    /// Returns the intent digest the receipt covers.
    #[must_use]
    pub fn intent_hash(&self) -> &str {
        &self.intent_hash
    }

    /// Returns the policy version that produced the decision.
    #[must_use]
    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }

    /// Returns the risk the decision was taken at.
    #[must_use]
    pub const fn risk_level(&self) -> u8 {
        self.risk_level
    }

    /// Returns the approval the authorization came from, when one was required.
    #[must_use]
    pub fn approval_id(&self) -> Option<&str> {
        self.approval_id.as_deref()
    }

    /// Returns the identity that decided the approval, when there was one.
    #[must_use]
    pub fn approver_id(&self) -> Option<&str> {
        self.approver_id.as_deref()
    }

    /// Returns when the approval was decided, when there was one.
    #[must_use]
    pub const fn approved_at(&self) -> Option<UtcTimestamp> {
        self.approved_at
    }

    /// Returns when the authority lapses, when it came from an approval.
    #[must_use]
    pub const fn expires_at(&self) -> Option<UtcTimestamp> {
        self.expires_at
    }

    /// Returns the correlation identity.
    #[must_use]
    pub const fn correlation_id(&self) -> CorrelationId {
        self.correlation_id
    }

    /// Returns when the receipt was issued.
    #[must_use]
    pub const fn issued_at(&self) -> UtcTimestamp {
        self.issued_at
    }

    /// Returns whether the receipt's authority was still valid at an instant.
    ///
    /// A receipt without an expiry is authority policy granted directly, which does not lapse by
    /// itself. A receipt citing an approval lapses exactly when that approval does, and the comparison
    /// is in Rust from `unix_nanos` because the stored timestamp form does not sort
    /// lexicographically (`ADR-0018`).
    #[must_use]
    pub fn is_valid_at(&self, now: UtcTimestamp) -> bool {
        match self.expires_at {
            Some(expires_at) => now.unix_nanos() < expires_at.unix_nanos(),
            None => true,
        }
    }
}

/// What one adapter call established.
///
/// The three fields are the three answers a caller needs and must not conflate: what happened
/// ([`ToolOutcome`]), what proves it ([`ProviderEvidence`]), and what the tool produced
/// ([`BoundedOutput`]).
///
/// # Why there is no constructor that infers the outcome
///
/// An adapter that fails has to say whether the request reached the provider. Nothing outside the
/// adapter can know, and a constructor that guessed would either report a sent request as `Failed`
/// (inviting a retry that duplicates an effect) or a never-sent request as `Unknown` (refusing a safe
/// retry). Both guesses are worse than requiring the adapter to answer, so the outcome is a parameter
/// everywhere.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCallResult {
    record: ToolOutcomeRecord,
    evidence: Option<ProviderEvidence>,
    output: Option<BoundedOutput>,
    reported_at: UtcTimestamp,
}

impl ToolCallResult {
    /// Records what a call established.
    ///
    /// `record` is built by [`ToolOutcomeRecord`], which already refuses a `Confirmed` outcome with no
    /// evidence and a `Failed` outcome with no reason — so the honesty rules are enforced by the type
    /// this constructor takes rather than re-checked here. `evidence` is the provider locator, carried
    /// separately from the outcome's own `reason` so a success is never conflated with a diagnostic.
    #[must_use]
    pub const fn new(
        record: ToolOutcomeRecord,
        evidence: Option<ProviderEvidence>,
        output: Option<BoundedOutput>,
        reported_at: UtcTimestamp,
    ) -> Self {
        Self {
            record,
            evidence,
            output,
            reported_at,
        }
    }

    /// Returns the outcome state.
    #[must_use]
    pub const fn outcome(&self) -> ToolOutcome {
        self.record.outcome()
    }

    /// Returns the outcome record, which carries the reason for a failure or the evidence for a
    /// confirmation.
    #[must_use]
    pub const fn record(&self) -> &ToolOutcomeRecord {
        &self.record
    }

    /// Returns the provider's locator for the effect.
    #[must_use]
    pub const fn evidence(&self) -> Option<&ProviderEvidence> {
        self.evidence.as_ref()
    }

    /// Returns the bounded tool output.
    #[must_use]
    pub const fn output(&self) -> Option<&BoundedOutput> {
        self.output.as_ref()
    }

    /// Returns when the adapter reported.
    #[must_use]
    pub const fn reported_at(&self) -> UtcTimestamp {
        self.reported_at
    }

    /// Returns whether the call reached a terminal outcome.
    ///
    /// A non-terminal result is a legitimate report — `Submitted` means the provider accepted the
    /// request and the effect is not yet confirmed — so a caller must be able to tell that a stored
    /// call is not finished rather than treating every stored result as final.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        self.record.outcome().is_terminal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::risk::Risk;

    /// A bounded output keeps small content, unmarked.
    #[test]
    fn a_small_output_is_kept() {
        let output = BoundedOutput::truncating("hello");
        assert_eq!(output.content(), "hello");
        assert!(!output.is_truncated());
        assert_eq!(output.byte_len(), 5);
        assert_eq!(output.to_string(), "hello");

        let strict = BoundedOutput::strict("hello").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(strict, output);
    }

    /// **An oversized output is truncated on a character boundary and marked.**
    ///
    /// The boundary half is the part a byte slice would get wrong: cutting inside a multi-byte
    /// character panics in Rust rather than producing invalid text, so this asserts the character
    /// count is preserved and the content is still valid.
    #[test]
    fn an_oversized_output_is_truncated_on_a_character_boundary() {
        let output = BoundedOutput::truncating("é".repeat(MAX_TOOL_OUTPUT_BYTES));
        assert!(output.is_truncated());
        assert!(output.byte_len() <= MAX_TOOL_OUTPUT_BYTES);
        assert_eq!(
            output.byte_len() % 2,
            0,
            "no multi-byte character may be split"
        );
        assert_eq!(
            output.byte_len(),
            MAX_TOOL_OUTPUT_BYTES,
            "an even-sized character fills the bound exactly"
        );
        assert!(output.content().chars().all(|c| c == 'é'));

        // A three-byte character cannot fill it exactly, so the bound is approached, not met.
        let uneven = BoundedOutput::truncating("✅".repeat(MAX_TOOL_OUTPUT_BYTES));
        assert!(uneven.is_truncated());
        assert!(uneven.byte_len() < MAX_TOOL_OUTPUT_BYTES);
        assert!(uneven.byte_len() > MAX_TOOL_OUTPUT_BYTES - 4);
        assert!(uneven.content().chars().all(|c| c == '✅'));

        // The marker is a flag, so it cannot be confused with the tool's own content.
        let deceptive = BoundedOutput::truncating("[output truncated]");
        assert!(!deceptive.is_truncated());
        assert!(
            !BoundedOutput::truncating("x".repeat(MAX_TOOL_OUTPUT_BYTES + 1))
                .content()
                .contains("truncated"),
            "the content must not gain text"
        );
    }

    /// A strict output refuses rather than truncating.
    #[test]
    fn a_strict_output_refuses_an_oversized_value() {
        assert_eq!(
            BoundedOutput::strict("x".repeat(MAX_TOOL_OUTPUT_BYTES + 1)),
            Err(OutputError::TooLarge)
        );
        // The bound itself is accepted, so it is a bound and not an off-by-one.
        assert!(BoundedOutput::strict("x".repeat(MAX_TOOL_OUTPUT_BYTES)).is_ok());
        // A multi-byte value just over the bound is also refused, since the bound is in bytes.
        assert!(BoundedOutput::strict("é".repeat(MAX_TOOL_OUTPUT_BYTES)).is_err());
    }

    /// Provider evidence must be a real, bounded locator.
    #[test]
    fn evidence_must_be_a_real_locator() {
        assert_eq!(ProviderEvidence::new(""), Err(EvidenceError::Unusable));
        assert_eq!(ProviderEvidence::new("   "), Err(EvidenceError::Unusable));
        assert_eq!(
            ProviderEvidence::new("x".repeat(MAX_PROVIDER_EVIDENCE_CHARS + 1)),
            Err(EvidenceError::Unusable)
        );
        let evidence =
            ProviderEvidence::new("  message-id-12345  ").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(evidence.as_str(), "message-id-12345", "trimmed");
        assert_eq!(evidence.to_string(), "message-id-12345");
        assert!(ProviderEvidence::new("x".repeat(MAX_PROVIDER_EVIDENCE_CHARS)).is_ok());
    }

    /// An idempotency key generates, is fixed-width, and parses back.
    #[test]
    fn an_idempotency_key_round_trips() {
        let key = IdempotencyKey::generate().unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(key.as_str().len(), 32);
        assert!(
            key.as_str()
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        );
        assert_eq!(
            IdempotencyKey::parse(key.as_str()).unwrap_or_else(|error| panic!("{error}")),
            key
        );

        // Two keys differ, so generation is not a constant.
        let other = IdempotencyKey::generate().unwrap_or_else(|error| panic!("{error}"));
        assert_ne!(key, other);

        // Case is normalized, so a stored key is not case-dependent.
        let upper = key.as_str().to_uppercase();
        assert_eq!(
            IdempotencyKey::parse(&upper)
                .unwrap_or_else(|error| panic!("{error}"))
                .as_str(),
            key.as_str()
        );
    }

    /// A malformed idempotency key is refused.
    #[test]
    fn a_malformed_idempotency_key_is_refused() {
        assert_eq!(IdempotencyKey::parse(""), Err(IdempotencyKeyError::Length));
        assert_eq!(
            IdempotencyKey::parse(&"a".repeat(31)),
            Err(IdempotencyKeyError::Length)
        );
        assert_eq!(
            IdempotencyKey::parse(&"a".repeat(33)),
            Err(IdempotencyKeyError::Length)
        );
        assert_eq!(
            IdempotencyKey::parse(&"z".repeat(32)),
            Err(IdempotencyKeyError::NotHexadecimal)
        );
        // The serialized form round-trips through the string, which is what storage needs.
        let key = IdempotencyKey::generate().unwrap_or_else(|error| panic!("{error}"));
        let encoded = serde_json::to_string(&key).unwrap_or_else(|error| panic!("{error}"));
        let decoded: IdempotencyKey =
            serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, key);
    }

    fn receipt_parts(approval: Option<ApprovalCitation>) -> AuthorizationReceiptParts {
        AuthorizationReceiptParts {
            receipt_id: "0198f000-0000-7000-8000-0000000000e1".to_owned(),
            tool: ToolId::new("jarvis.mail.send").unwrap_or_else(|error| panic!("{error}")),
            tool_version: "1.0.0".to_owned(),
            intent_hash: "a".repeat(64),
            policy_version: "policy-3".to_owned(),
            risk_level: Risk::Moderate,
            approval,
            correlation_id: CorrelationId::new(),
            issued_at: UtcTimestamp::from_unix_nanos(1_774_000_000_000_000_000)
                .unwrap_or_else(|error| panic!("{error}")),
        }
    }

    fn citation() -> ApprovalCitation {
        ApprovalCitation {
            approval_id: "0198f000-0000-7000-8000-0000000000e2".to_owned(),
            approver_id: "user-2".to_owned(),
            approved_at: UtcTimestamp::from_unix_nanos(1_774_000_000_000_000_000)
                .unwrap_or_else(|error| panic!("{error}")),
            expires_at: UtcTimestamp::from_unix_nanos(1_774_000_600_000_000_000)
                .unwrap_or_else(|error| panic!("{error}")),
        }
    }

    /// A receipt without an approval carries no approval fields at all.
    #[test]
    fn a_direct_authorization_carries_no_approval() {
        let receipt = AuthorizationReceipt::new(receipt_parts(None))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(receipt.tool(), "jarvis.mail.send");
        assert_eq!(receipt.tool_version(), "1.0.0");
        assert_eq!(receipt.policy_version(), "policy-3");
        assert_eq!(receipt.risk_level(), 2);
        assert_eq!(receipt.approval_id(), None);
        assert_eq!(receipt.approver_id(), None);
        assert_eq!(receipt.approved_at(), None);
        assert_eq!(receipt.expires_at(), None);
        assert!(
            receipt.is_valid_at(
                UtcTimestamp::from_unix_nanos(1_900_000_000_000_000_000)
                    .unwrap_or_else(|error| panic!("{error}"))
            ),
            "authority granted directly does not lapse by itself"
        );
    }

    /// **An approval citation fills all four fields, so a half-cited approval is unrepresentable.**
    #[test]
    fn an_approval_citation_fills_every_approval_field() {
        let receipt = AuthorizationReceipt::new(receipt_parts(Some(citation())))
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            receipt.approval_id(),
            Some("0198f000-0000-7000-8000-0000000000e2")
        );
        assert_eq!(receipt.approver_id(), Some("user-2"));
        assert!(receipt.approved_at().is_some());
        assert!(receipt.expires_at().is_some());

        let before = UtcTimestamp::from_unix_nanos(1_774_000_500_000_000_000)
            .unwrap_or_else(|error| panic!("{error}"));
        let at = UtcTimestamp::from_unix_nanos(1_774_000_600_000_000_000)
            .unwrap_or_else(|error| panic!("{error}"));
        let after = UtcTimestamp::from_unix_nanos(1_774_000_700_000_000_000)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(receipt.is_valid_at(before));
        assert!(
            !receipt.is_valid_at(at),
            "authority lapses exactly when the approval does"
        );
        assert!(!receipt.is_valid_at(after));
    }

    /// An empty approver inside a citation is refused.
    #[test]
    fn a_citation_without_an_approver_is_refused() {
        let mut broken = citation();
        broken.approver_id = "   ".to_owned();
        assert_eq!(
            AuthorizationReceipt::new(receipt_parts(Some(broken))),
            Err(ReceiptError::ApprovalIncomplete)
        );
    }

    /// An unusable policy version is refused.
    #[test]
    fn an_unusable_policy_version_is_refused() {
        for version in ["", "   ", &"x".repeat(MAX_POLICY_VERSION_CHARS + 1)] {
            let mut parts = receipt_parts(None);
            parts.policy_version = version.to_owned();
            assert_eq!(
                AuthorizationReceipt::new(parts),
                Err(ReceiptError::PolicyVersion),
                "{version:?} must be unusable"
            );
        }
    }

    /// A receipt round-trips through serialization, which storage needs.
    #[test]
    fn a_receipt_round_trips() {
        let receipt = AuthorizationReceipt::new(receipt_parts(Some(citation())))
            .unwrap_or_else(|error| panic!("{error}"));
        let encoded = serde_json::to_string(&receipt).unwrap_or_else(|error| panic!("{error}"));
        let decoded: AuthorizationReceipt =
            serde_json::from_str(&encoded).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(decoded, receipt);
    }

    /// A result carries the three answers separately.
    #[test]
    fn a_result_carries_the_outcome_evidence_and_output_separately() {
        let at = UtcTimestamp::from_unix_nanos(1_774_000_000_000_000_000)
            .unwrap_or_else(|error| panic!("{error}"));
        let confirmed =
            ToolOutcomeRecord::confirmed("message-id-9").unwrap_or_else(|error| panic!("{error}"));
        let evidence =
            ProviderEvidence::new("message-id-9").unwrap_or_else(|error| panic!("{error}"));
        let output = BoundedOutput::truncating("sent");
        let result = ToolCallResult::new(confirmed, Some(evidence), Some(output), at);

        assert_eq!(result.outcome(), ToolOutcome::Confirmed);
        assert!(result.is_terminal());
        assert_eq!(
            result.evidence().map(ProviderEvidence::as_str),
            Some("message-id-9")
        );
        assert_eq!(result.output().map(BoundedOutput::content), Some("sent"));
        assert_eq!(result.reported_at(), at);

        // The outcome's own record and the provider evidence are distinct fields.
        assert_eq!(result.record().evidence(), Some("message-id-9"));
    }

    /// **`unknown` and `failed` differ on both questions a caller decides with.**
    ///
    /// The distinction `docs/architecture/tools-and-connectors.md` draws, asserted on the two
    /// predicates rather than on the variant name:
    ///
    /// - `unknown` means an effect **may have happened** and must **not** be repeated blindly;
    /// - `failed` means no effect happened, so a repeat is safe.
    ///
    /// Getting these backwards is the bug that turns one sent message into two, so both directions are
    /// asserted. An earlier version of this test asserted the opposite of the documented semantics —
    /// that `unknown` had not had an effect — which would have been exactly the inversion.
    #[test]
    fn a_unknown_outcome_is_distinct_from_a_failure() {
        let at = UtcTimestamp::from_unix_nanos(1_774_000_000_000_000_000)
            .unwrap_or_else(|error| panic!("{error}"));
        let unknown = ToolCallResult::new(
            ToolOutcomeRecord::new(ToolOutcome::Unknown).unwrap_or_else(|error| panic!("{error}")),
            None,
            None,
            at,
        );
        assert_eq!(unknown.outcome(), ToolOutcome::Unknown);
        assert!(unknown.is_terminal());
        assert!(
            unknown.outcome().may_have_had_an_effect(),
            "unknown means the effect may have happened"
        );
        assert!(
            !unknown.outcome().is_safe_to_repeat_from_outcome(),
            "and so it must not be repeated blindly"
        );

        let failed = ToolCallResult::new(
            ToolOutcomeRecord::failed("the provider refused the request")
                .unwrap_or_else(|error| panic!("{error}")),
            None,
            None,
            at,
        );
        assert_eq!(failed.outcome(), ToolOutcome::Failed);
        assert!(
            !failed.outcome().may_have_had_an_effect(),
            "failed means no effect happened"
        );
        assert!(
            failed.outcome().is_safe_to_repeat_from_outcome(),
            "so a repeat is safe from the outcome alone"
        );
        assert_ne!(unknown.outcome(), failed.outcome());
        assert_eq!(
            failed.record().reason(),
            Some("the provider refused the request")
        );

        // `confirmed` is proof, so it is neither repeatable nor merely possible.
        let confirmed = ToolCallResult::new(
            ToolOutcomeRecord::confirmed("provider-id-1").unwrap_or_else(|error| panic!("{error}")),
            None,
            None,
            at,
        );
        assert!(!confirmed.outcome().may_have_had_an_effect());
        assert!(!confirmed.outcome().is_safe_to_repeat_from_outcome());

        // A submitted result is NOT terminal, which is the distinction a stored call needs.
        let submitted = ToolCallResult::new(
            ToolOutcomeRecord::new(ToolOutcome::Submitted)
                .unwrap_or_else(|error| panic!("{error}")),
            None,
            None,
            at,
        );
        assert!(!submitted.is_terminal());
        assert!(submitted.outcome().reached_provider());
        assert!(submitted.outcome().may_have_had_an_effect());
    }
}
