//! Webhooks: how a provider event is authenticated, bound, and refused.
//!
//! `security.md`'s threat table names the attacks this module exists for:
//!
//! | Threat | Control |
//! | --- | --- |
//! | Webhook spoof/replay | raw-body signature verification, timestamp window, endpoint binding, dedupe key, durable inbox |
//!
//! And `tools-and-connectors.md` lists "webhook/subscription lifecycle" as connector-owned, with a completion
//! gate requiring "webhook signature/replay tests if applicable".
//!
//! # Why this is a contract and not a verifier
//!
//! There is no HMAC in this module. A verifier would need the signing secret, and a signing secret is the one
//! credential a connector compares rather than sends ([`crate::manifest::SecretKind::SigningSecret`]) — so a
//! type that could verify would be a type that held the secret, which the manifest's own secret model exists
//! to prevent. What this module owns is the **decision**: whether a delivery is acceptable, and which of the
//! security controls refused it. `P5-002` supplies the comparison and calls this.
//!
//! # Everything here checks *before* parsing
//!
//! `A12` (a `P6` case, whose contract is nonetheless fixed here) says a delivery that fails a check must be
//! "rejected before trusted parsing/workflow execution". The ordering is the security property: a signature
//! over a **re-serialized** body verifies a different document than the one that arrived, which is how a
//! JSON parser's key-order or duplicate-key behaviour becomes an injection. So [`WebhookDelivery`] carries the
//! values a control needs and nothing that has been interpreted.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The longest accepted signature header.
pub const MAX_SIGNATURE_HEADER_CHARS: usize = 512;

/// The most seconds a delivery's timestamp may differ from now.
///
/// Five minutes, which is the value every major provider's own documentation converges on and the one
/// `security.md` means by "timestamp window". It is a **bound on a replay window**, so it trades two failures
/// against each other: too short and a delivery delayed by a retrying provider is refused, too long and a
/// captured delivery can be replayed for longer. Five minutes is generous for network delay and short enough
/// that a captured body is stale before an attacker can use it.
pub const MAX_TIMESTAMP_SKEW_SECONDS: u64 = 300;

/// The longest accepted replay window.
///
/// One hour, and larger than the skew window deliberately: a **replay window** is how long a delivery's
/// dedupe key is remembered, and a provider may legitimately redeliver a failed delivery after a delay that
/// exceeds the skew. The two are different questions — "is this timestamp plausible" versus "have I seen this
/// delivery" — and conflating them would either refuse legitimate retries or allow a replay.
pub const MAX_REPLAY_WINDOW_SECONDS: u64 = 3_600;

/// The signature algorithm a provider uses.
///
/// A closed set, and `None` exists because a provider may authenticate deliveries another way (a bearer
/// token in a header, an IP allowlist). Representable rather than absent, because a manifest that omitted the
/// scheme would be indistinguishable from one whose author did not consider the question — the same reasoning
/// as [`crate::manifest::WebhookSupport::Unsupported`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureAlgorithm {
    /// HMAC-SHA256 over the raw body. The common case.
    HmacSha256,
    /// HMAC-SHA1, which some providers still use. Weaker, so named rather than folded in.
    HmacSha1,
    /// An Ed25519 signature, verified with a public key.
    ///
    /// Unlike the HMAC variants this is an **asymmetric** signature, so the verifier holds a public key and
    /// the provider holds the private one — which is why [`is_keyed_mac`](Self::is_keyed_mac) is false for it
    /// even though it also authenticates the delivered bytes. The two questions are separate: a keyed MAC and
    /// an asymmetric signature both cover the body, but only the MAC requires the verifier to hold a secret.
    /// A scheme whose *input* is unclear — a signature over parsed-and-reserialized fields, or over a
    /// different body than the one delivered — is **not** representable, and that limit is deliberate rather
    /// than an omission: naming such an algorithm would invite a verifier to compare the wrong bytes.
    Ed25519,
    /// The provider supplies no signature, so another mechanism must bind the delivery.
    ///
    /// Refused for a [`WebhookSupport::Push`](crate::manifest::WebhookSupport::Push) declaration by
    /// `ConnectorManifest`'s webhook validation, because a push endpoint whose deliveries are unauthenticated
    /// is an unauthenticated write into the platform — the "webhook spoof" row of `security.md`'s table with
    /// its control removed. Representable rather than absent so that an author must **state** the absence; a
    /// scheme that omitted it would be indistinguishable from one whose author never considered the question.
    None,
}

impl SignatureAlgorithm {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HmacSha256 => "hmac_sha256",
            Self::HmacSha1 => "hmac_sha1",
            Self::Ed25519 => "ed25519",
            Self::None => "none",
        }
    }

    /// Returns whether the algorithm is a keyed MAC over the raw body.
    ///
    /// Both HMAC variants are; [`Self::Ed25519`] is not, because it is verified with a **public** key rather
    /// than a shared secret. The property it asks about is "does verifying require a secret", which is what a
    /// provider's onboarding and a secret-storage decision depend on — it is *not* "does this cover the raw
    /// bytes", which both HMAC and Ed25519 do and a signature over a parsed structure does not.
    #[must_use]
    pub const fn is_keyed_mac(self) -> bool {
        matches!(self, Self::HmacSha256 | Self::HmacSha1)
    }
}

/// How a provider's signature is presented.
///
/// The header name and encoding are provider-specific and both matter: a verifier that read the wrong header
/// would find nothing and refuse every delivery, and one that decoded base64 when the provider sent hex would
/// compare two different strings and refuse every delivery too — a failure that looks like a wrong secret.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SignatureScheme {
    /// The algorithm.
    pub algorithm: SignatureAlgorithm,
    /// The header the signature arrives in. Lowercase, because HTTP header names are case-insensitive and a
    /// case-sensitive comparison would refuse a conforming sender.
    pub header: String,
    /// The encoding the signature is presented in.
    pub encoding: SignatureEncoding,
}

/// How a signature's bytes are encoded in its header.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureEncoding {
    /// Lowercase hexadecimal.
    Hex,
    /// Base64, standard alphabet.
    Base64,
    /// Base64url, no padding.
    Base64Url,
    /// The provider's header holds the raw signature with no encoding.
    Raw,
}

impl SignatureEncoding {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hex => "hex",
            Self::Base64 => "base64",
            Self::Base64Url => "base64url",
            Self::Raw => "raw",
        }
    }
}

impl SignatureScheme {
    /// Validates a signature scheme.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureError::Header`] when the header name is empty, not a valid HTTP token, or not
    /// lowercase. The token-character check is what stops a header name from carrying a colon or a space,
    /// which would make the lookup silently find nothing — a failure whose symptom is "every signature is
    /// wrong", one step away from the real cause.
    pub fn new(
        algorithm: SignatureAlgorithm,
        header: impl Into<String>,
        encoding: SignatureEncoding,
    ) -> Result<Self, SignatureError> {
        let header = header.into();
        if header.is_empty() || header.len() > 64 {
            return Err(SignatureError::Header {
                reason: "a signature header name must be 1 to 64 characters",
            });
        }
        // RFC 9110's token characters, minus uppercase: a header name is case-insensitive on the wire, so
        // storing it lowercase makes a comparison in this crate agree with the wire's own rule.
        let valid = header.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.' | b'+')
        });
        if !valid {
            return Err(SignatureError::Header {
                reason: "a signature header name must be lowercase and hold only token characters, so a \
                         lookup agrees with the wire's case-insensitive rule",
            });
        }
        Ok(Self {
            algorithm,
            header,
            encoding,
        })
    }

    /// Returns whether this scheme authenticates a delivery.
    #[must_use]
    pub const fn authenticates(&self) -> bool {
        !matches!(self.algorithm, SignatureAlgorithm::None)
    }
}

/// How a delivery is bound to one account and one endpoint.
///
/// `security.md`'s "endpoint binding" control, and the requirement is doing two jobs: it stops a delivery
/// intended for one account being applied to another, and it stops a **valid** delivery captured from one
/// deployment being replayed against another that shares a signing secret. Both are real because a provider
/// typically signs with one secret per application, not one per subscriber.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WebhookBinding {
    /// The path a delivery must arrive at.
    ///
    /// Exact rather than a prefix: `A12` names "wrong endpoint" as a rejection reason, and a prefix match
    /// would admit `/webhooks/google/anything` for a binding of `/webhooks/google`.
    pub path: String,
    /// The header naming the account, when the provider supplies one.
    ///
    /// Optional because not every provider does. When present, the delivery's value must resolve to the
    /// account the connector believes it is — which is how a delivery for a removed account is refused rather
    /// than applied to whatever account remains.
    pub account_header: Option<String>,
    /// Whether the provider's payload states the account, as a fallback binding.
    ///
    /// Some providers put the account only inside the body, which means the binding cannot be checked before
    /// parsing — a real tension with `A12`'s "before trusted parsing". Recorded rather than hidden: with this
    /// true, the binding check happens after a **structural** parse (enough to read one field) and before any
    /// workflow dispatch, and that ordering is what `P6` must implement.
    pub account_in_body: bool,
}

impl WebhookBinding {
    /// Validates a binding.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureError::Binding`] when the path is empty, does not start with `/`, holds a query or
    /// fragment, or contains a `..` segment — and when **neither** an account header nor a body field is
    /// declared, because then nothing binds a delivery to an account and the signature is the only control.
    pub fn new(
        path: impl Into<String>,
        account_header: Option<String>,
        account_in_body: bool,
    ) -> Result<Self, SignatureError> {
        let path = path.into();
        if path.is_empty() || !path.starts_with('/') {
            return Err(SignatureError::Binding {
                reason: "a webhook path must be absolute",
            });
        }
        if path.contains('?') || path.contains('#') {
            return Err(SignatureError::Binding {
                reason: "a webhook path may not carry a query or fragment; a delivery arrives at a path, so \
                         a query would never match and every delivery would be refused with no clue why",
            });
        }
        if path.split('/').any(|segment| segment == "..") {
            return Err(SignatureError::Binding {
                reason: "a webhook path may not contain `..`",
            });
        }
        let has_header = account_header
            .as_deref()
            .is_some_and(|header| !header.trim().is_empty());
        if !has_header && !account_in_body {
            return Err(SignatureError::Binding {
                reason: "a binding must name the account either in a header or in the body; with neither, a \
                         valid delivery for one account could be applied to another",
            });
        }
        Ok(Self {
            path,
            account_header: account_header.filter(|header| !header.trim().is_empty()),
            account_in_body,
        })
    }

    /// Returns whether the binding can be checked from headers alone.
    ///
    /// The property `A12`'s "before trusted parsing" needs. False when the account is only in the body, and
    /// a caller that cannot check a binding before parsing must still check it before **dispatching** — which
    /// is why this predicate exists rather than the situation being left implicit.
    #[must_use]
    pub const fn is_checkable_before_parsing(&self) -> bool {
        self.account_header.is_some()
    }
}

/// Why a delivery was refused.
///
/// Every variant is a distinct control, and they are separate because `A12` enumerates them: "Mutated body,
/// stale timestamp, wrong endpoint/account, invalid signature, oversized payload, and replay outside policy
/// are rejected before trusted parsing/workflow execution." A single `Rejected` variant would make an
/// operator unable to tell an attack from a misconfiguration — an invalid signature is an attacker or a
/// wrong secret, while a wrong endpoint is almost always a deployment mistake.
///
/// The refusal carries **no detail about the expected value**, deliberately: on the wire a reason is returned
/// only to the provider, which is not the party to help, and a diagnostic naming the expected timestamp or
/// account would help an attacker probe. `P3-009`'s refusal-names-the-policy-class-only rule applies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebhookRejection {
    /// The body's bytes do not match the signature.
    ///
    /// The one refusal where an attacker and a wrong secret are indistinguishable, which is why the operator
    /// diagnostic says both possibilities rather than asserting one — `P3-008i`'s rule that a message must not
    /// assert a cause it cannot know.
    InvalidSignature,
    /// The delivery's timestamp is outside the accepted window.
    StaleTimestamp,
    /// The delivery's timestamp is in the future beyond the skew allowance.
    ///
    /// Separate from [`Self::StaleTimestamp`] because the causes differ: a stale one is a replay, while a
    /// future one is a clock difference between the provider and this host, which is a configuration problem.
    FutureTimestamp,
    /// The delivery arrived at a path the binding does not name.
    WrongEndpoint,
    /// The delivery names an account this connector does not have.
    WrongAccount,
    /// The delivery exceeds the accepted payload size.
    Oversized,
    /// The delivery was seen before, within the replay window.
    ///
    /// The dedupe control. Reported as a **refusal** rather than silently accepted, because `A12` requires
    /// that a duplicate concurrent delivery produces "one canonical event/effect" — so the second arrival must
    /// be identifiable as a duplicate rather than being processed and deduplicated later.
    Replayed,
    /// The payload declares a schema version this connector does not handle.
    ///
    /// `events-and-workflows.md` requires a "schema version" on the event envelope, and a provider's payload
    /// is one. Refusing an unknown version keeps the failure visible instead of a partial parse producing a
    /// half-populated event.
    UnknownSchemaVersion,
}

impl WebhookRejection {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSignature => "invalid_signature",
            Self::StaleTimestamp => "stale_timestamp",
            Self::FutureTimestamp => "future_timestamp",
            Self::WrongEndpoint => "wrong_endpoint",
            Self::WrongAccount => "wrong_account",
            Self::Oversized => "oversized",
            Self::Replayed => "replayed",
            Self::UnknownSchemaVersion => "unknown_schema_version",
        }
    }

    /// Returns whether the refusal indicates the sender is not the provider.
    ///
    /// The distinction that decides whether a refusal is worth an alert: an invalid signature or a wrong
    /// account is either an attacker or a broken deployment and deserves attention, while a `Replayed`
    /// delivery is a provider doing exactly what it promised and must not page anyone. `P4-006`'s
    /// `SemanticAbsence` made the same distinction — four operational answers must not all read as `0`.
    #[must_use]
    pub const fn indicates_an_authenticity_failure(self) -> bool {
        matches!(self, Self::InvalidSignature | Self::WrongAccount)
    }

    /// Returns whether a retry by the provider could succeed.
    ///
    /// True only for [`Self::Replayed`] — there is nothing to retry, but answering with a success tells the
    /// provider to stop, which is the correct response to a duplicate. Everything else answers with a failure
    /// so the provider's own retry policy applies, except an oversize or an unknown schema version, which will
    /// fail identically.
    #[must_use]
    pub const fn should_acknowledge_to_the_provider(self) -> bool {
        matches!(self, Self::Replayed)
    }
}

/// A delivery as it arrived, with the values a control needs and nothing interpreted.
///
/// # Why the body is bytes
///
/// The signature is over the **raw bytes**. A type whose body was a `String` or a parsed `Value` would make
/// the re-serialization defect representable — a verifier that re-serialized could produce different bytes
/// for the same document, and JSON permits several encodings of one value (key order, escapes, whitespace,
/// duplicate keys). Carrying `&[u8]` makes "verify what arrived" the only thing a verifier can do.
#[derive(Clone, Copy, Debug)]
pub struct WebhookDelivery<'a> {
    /// The path the delivery arrived at.
    pub path: &'a str,
    /// The header values a control needs, as raw byte slices.
    ///
    /// A slice of `(name, value)` pairs rather than a map, because a **duplicate** header is meaningful: a
    /// sender that supplies two signatures is either a proxy doing what proxies do or an attacker trying to
    /// get one of them checked. Collapsing duplicates into a map would hide the second, and this type keeps
    /// both so a verifier can refuse the ambiguity.
    pub headers: &'a [(&'a str, &'a [u8])],
    /// The raw body.
    pub body: &'a [u8],
}

impl<'a> WebhookDelivery<'a> {
    /// Returns every value for a header, case-insensitively.
    ///
    /// Returns a slice rather than an `Option`, so a caller that expects one value can **notice** a duplicate
    /// rather than silently taking the first. HTTP header names are case-insensitive, so the comparison
    /// lowercases each name — with `eq_ignore_ascii_case` rather than allocating a lowercased copy.
    #[must_use]
    pub fn header_values(&self, name: &str) -> Vec<&'a [u8]> {
        self.headers
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| *value)
            .collect()
    }

    /// Returns a header's value as UTF-8, when exactly one is present and it is valid UTF-8.
    ///
    /// `None` for **zero or several** values, which is deliberate: a caller that treated an ambiguous header
    /// as absent would proceed without it, and one that took the first would ignore the second. Both are
    /// wrong in a security check, so neither is possible — the caller must decide what an ambiguous header
    /// means for its own control.
    #[must_use]
    pub fn single_header(&self, name: &str) -> Option<&'a str> {
        let values = self.header_values(name);
        match values.as_slice() {
            [value] => std::str::from_utf8(value).ok(),
            _ => None,
        }
    }

    /// Returns whether the body is within the accepted size.
    #[must_use]
    pub fn is_within_payload_bound(&self, maximum_bytes: usize) -> bool {
        self.body.len() <= maximum_bytes
    }
}

/// The replay window a delivery is checked against.
///
/// A value rather than a constant, because the window is a **policy** decision: `security.md` requires
/// "dedupe key, durable inbox" and the window is how long one is remembered. Validated above zero so that a
/// zero window cannot disable replay detection while reading as configured — the failure direction that
/// matters, since a disabled control still looks enabled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplayWindow(u64);

impl ReplayWindow {
    /// Validates a replay window.
    ///
    /// # Errors
    ///
    /// Returns [`SignatureError::ReplayWindow`] when the value is zero or above
    /// [`MAX_REPLAY_WINDOW_SECONDS`].
    pub fn new(seconds: u64) -> Result<Self, SignatureError> {
        if seconds == 0 || seconds > MAX_REPLAY_WINDOW_SECONDS {
            return Err(SignatureError::ReplayWindow {
                reason: "a replay window must be 1 to 3600 seconds; zero would disable replay detection \
                         while reading as configured",
            });
        }
        Ok(Self(seconds))
    }

    /// Returns the window in seconds.
    #[must_use]
    pub const fn seconds(self) -> u64 {
        self.0
    }

    /// Returns whether the window is longer than the timestamp skew allowance.
    ///
    /// Reported because the two interact: a replay window **shorter** than the skew window leaves a gap in
    /// which a delivery is still inside its timestamp allowance but has been forgotten, so a replay inside
    /// that gap is accepted. `P4-009`'s lesson about two bounds belonging to different things applies — the
    /// relationship is a fact worth stating rather than an invariant to assume.
    #[must_use]
    pub const fn covers_the_skew_allowance(self) -> bool {
        self.0 >= MAX_TIMESTAMP_SKEW_SECONDS
    }
}

impl fmt::Display for ReplayWindow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}s", self.0)
    }
}

/// Why a webhook declaration is unusable.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SignatureError {
    /// The signature header name is unusable.
    #[error("the signature header is unusable: {reason}")]
    Header {
        /// What is wrong.
        reason: &'static str,
    },
    /// The delivery binding is unusable.
    #[error("the webhook binding is unusable: {reason}")]
    Binding {
        /// What is wrong.
        reason: &'static str,
    },
    /// The replay window is unusable.
    #[error("the replay window is unusable: {reason}")]
    ReplayWindow {
        /// What is wrong.
        reason: &'static str,
    },
    /// A delivery presented more than one value for a security-relevant header.
    ///
    /// A refusal rather than a choice: a sender presenting two signatures is either a proxy or an attacker,
    /// and picking one is the decision that lets a proxy's value shadow a provider's.
    #[error(
        "the delivery presented {count} values for the `{header}` header, and exactly one is required"
    )]
    AmbiguousHeader {
        /// The header.
        header: &'static str,
        /// How many values arrived.
        count: usize,
    },
    /// A delivery's body was not the shape a control needed.
    #[error("the delivery's `{field}` could not be read")]
    Malformed {
        /// The field that could not be read.
        field: &'static str,
    },
}

#[cfg(test)]
#[path = "webhook_tests.rs"]
mod tests;
