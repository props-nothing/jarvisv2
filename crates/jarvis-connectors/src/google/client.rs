//! Google's HTTP decisions, taken without a socket.
//!
//! # What this module decides, and what it deliberately does not do
//!
//! Every function here is a **pure function of a response**: a status and a reason become a retry decision, a
//! body becomes a page, a cursor becomes either an advanced cursor or a documented staleness signal. That is
//! the whole of it, and the reason is `repository-layout.md`'s dependency rule plus a concrete consequence:
//! this crate has **no HTTP stack**, so it cannot send a request, and pretending otherwise would mean adding a
//! client library to a crate whose entire value is that its rules are checkable as functions of their
//! arguments.
//!
//! `jarvis-models` established the shape — an adapter with a `Transport` trait, tested against a scripted
//! implementation. The transport binding for this connector is the next step and is named in the limits of the
//! `P5-005` entry in `TODO.md`; what is here is everything that does not need one.
//!
//! # The three mappings, and where each comes from
//!
//! 1. **Status and `reason` to an action.** Google's own error page names four distinct 403 reasons whose
//!    remedies differ, three unrelated causes behind one 429, and a backoff recipe starting at one second.
//!    [`classify`] is that table, and it exists because **a status-code-only classifier gets a real case
//!    wrong**: `domainPolicy` is a permanent, user-actionable refusal — "the domain administrators have
//!    disabled Gmail apps" — that a caller reading only `403` would retry forever against an administrator's
//!    decision.
//! 2. **A page to its successor.** Both APIs paginate with `pageToken`/`nextPageToken`, and a token is
//!    provider-issued text that becomes a request parameter and a log field, so [`next_page`] bounds it.
//! 3. **A cursor to its successor, or to a resync.** Gmail signals a stale `historyId` with an ordinary
//!    **HTTP 404** and Calendar with **410 Gone**; the first is the case worth naming, because `404` is also
//!    what an absent account returns and the two remedies are opposites.
//!
//! # What is not verified
//!
//! **No request has been sent to Google.** Every mapping is transcribed from the dated sources in
//! `docs/research/integrations/google.md` and is tested against fixtures **built from that record** rather than
//! captured from the wire. The fixtures are therefore evidence that the code implements the record, not
//! evidence that the record matches Google — and that distinction is the reason the manifest declares
//! `CompatibilityVerdict::Unverified`.

use serde::Deserialize;

use crate::manifest::ConnectorVersion;
use crate::ratelimit::{ProviderRequestId, RetryClass, RetryDecision, RetryGuidance};
use crate::{AccountReference, CursorError, SyncCursor, SyncCursorKind};
use jarvis_core::UtcTimestamp;

/// The host both APIs are served from.
pub const GOOGLE_API_HOST: &str = "www.googleapis.com";

/// Gmail's API base, version `v1`.
pub const GMAIL_API_BASE: &str = "https://www.googleapis.com/gmail/v1";

/// Calendar's API base, version `v3`.
pub const CALENDAR_API_BASE: &str = "https://www.googleapis.com/calendar/v3";

/// The largest `maxResults` Gmail honours on a list call.
///
/// Documented as capped at 500, and a bound rather than a preference: asking for more does not fail, it
/// silently returns fewer, so a caller that assumed it had the page size it requested would mis-plan a sync.
pub const GMAIL_MAX_RESULTS_CAP: u32 = 500;

/// The largest batch Gmail accepts.
///
/// Two documented facts, in tension: batching is what makes a full sync affordable, and "larger batch sizes
/// can trigger rate limiting". So this is the ceiling the provider states and a caller must *also* pace.
pub const GMAIL_BATCH_LIMIT: u32 = 50;

/// The longest page token accepted, in characters.
///
/// A page token becomes a query parameter, so an unbounded one is an unbounded request. The bound is generous
/// — Google's own tokens are far shorter — and it refuses rather than truncates, because a truncated token is
/// a *different* token that the provider would either reject or, worse, interpret as a valid earlier page.
pub const MAX_PAGE_TOKEN_CHARS: usize = 4_096;

/// The backoff ceiling Google's guidance names.
///
/// "`maximum_backoff` is typically 32 or 64 seconds. The appropriate value depends on the use case."
///
/// **`RetryGuidance::BackoffSeconds` states a starting delay, not a ceiling**, so this constant exists for a
/// caller's own retry loop and is *not* carried by the decision. That is a real limit rather than a design
/// preference: a decision that carried both would need a field the type does not have, and inventing one for a
/// single provider would put a Google fact into shared vocabulary.
pub const GOOGLE_MAX_BACKOFF_SECONDS: u32 = 64;

/// Google's documented retry floor.
///
/// "Start retry periods at least one second after the error." Every transient or throttled classification
/// here starts at this value rather than at zero, because a first retry at zero is the one the documentation
/// explicitly rules out.
pub const GOOGLE_RETRY_FLOOR_SECONDS: u32 = 1;

/// A Gmail error response body.
///
/// The shape Google documents: an `error` object with a numeric `code`, a `message`, and an `errors` array
/// whose first element carries the `reason`.
///
/// **Only `reason` is read.** The `message` is not a field of this type at all, which is the structural form of
/// `P3-008c`'s rule: a classification must not be derived from message text, and a type that cannot hold the
/// text cannot derive anything from it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct GmailErrorBody {
    /// The error object.
    pub error: GmailErrorObject,
}

/// The `error` object of a Gmail error response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct GmailErrorObject {
    /// The HTTP status, repeated in the body.
    pub code: u16,
    /// The machine-readable reasons.
    ///
    /// An array because the documented samples show one entry per error; `default` because a body without one
    /// is still a body, and a missing reason must degrade to "unclassified" rather than to a parse failure.
    #[serde(default)]
    pub errors: Vec<GmailErrorEntry>,
}

/// One entry of a Gmail error's `errors` array.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct GmailErrorEntry {
    /// The machine-readable reason, which is what a classifier switches on.
    ///
    /// Optional on purpose: the documentation's own samples do not all carry one, and an absent reason must
    /// leave the caller able to classify from the status rather than refusing to parse the body.
    #[serde(default)]
    pub reason: Option<String>,
}

impl GmailErrorBody {
    /// Returns the first reason the body states, if any.
    ///
    /// The **first** rather than a set, because the retry decision is one decision: a response naming several
    /// reasons has one remedy, and the most restrictive reading is the one to take. `P3-009d`'s "most
    /// consequential reason wins" is the same rule applied to exposure.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.error
            .errors
            .iter()
            .find_map(|entry| entry.reason.as_deref())
            .filter(|reason| !reason.trim().is_empty())
    }
}

/// The `reason` values Google's error documentation names.
///
/// A **closed set**, and that is the point: each variant corresponds to a documented cause with a documented
/// remedy, so a classifier cannot be extended by a response. An unrecognised reason string is
/// [`Self::Unrecognised`] rather than a new variant, which keeps the vocabulary a property of this crate
/// rather than of the provider's next release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GmailErrorReason {
    /// `dailyLimitExceeded` — the project's daily quota is spent.
    ///
    /// **Permanent**, which is the classification that matters most here: the documentation says the daily
    /// threshold of 80,000,000 units **cannot be raised**, and the error page's own advice is to "raise the
    /// quota in the Google Cloud project". So a retry cannot help and an operator may not be able to either.
    DailyLimitExceeded,
    /// `rateLimitExceeded` — the project's request rate.
    RateLimitExceeded,
    /// `userRateLimitExceeded` — one user's request rate.
    UserRateLimitExceeded,
    /// `domainPolicy` — the user's domain administrator disabled the app.
    ///
    /// The reason this enum exists. It arrives as `403`, exactly like the two throttling reasons above, and its
    /// remedy is a conversation with an administrator, not a retry.
    DomainPolicy,
    /// `authError` — an expired or invalid token, or a missing scope.
    AuthError,
    /// `backendError` — an unexpected server-side failure.
    BackendError,
    /// `badRequest` — the request itself is unusable.
    BadRequest,
    /// A reason this crate does not know.
    ///
    /// Representable because Google adds reasons, and refusing to parse an unknown one would turn "a new error
    /// code" into "a connector that cannot read its own errors". The classification is the **status's**, so an
    /// unknown reason never *loosens* a decision.
    Unrecognised,
}

impl GmailErrorReason {
    /// Parses a reason string.
    ///
    /// An unrecognised string is [`Self::Unrecognised`] rather than an error, because a reason is data and a
    /// connector must be able to report an error it does not have a case for.
    #[must_use]
    pub fn parse(reason: &str) -> Self {
        // Matched on the value as written. A case-insensitive match would accept `DailyLimitExceeded`, which
        // the provider never sends, and silently fold it into the documented case.
        match reason {
            "dailyLimitExceeded" => Self::DailyLimitExceeded,
            "rateLimitExceeded" => Self::RateLimitExceeded,
            "userRateLimitExceeded" => Self::UserRateLimitExceeded,
            "domainPolicy" => Self::DomainPolicy,
            "authError" => Self::AuthError,
            "backendError" => Self::BackendError,
            "badRequest" => Self::BadRequest,
            _ => Self::Unrecognised,
        }
    }

    /// Returns the reason as Google spells it, or `None` for [`Self::Unrecognised`].
    ///
    /// `None` rather than the caller's original text, because this type does not retain it — a reason read
    /// from a response is provider-supplied text, and the only thing a decision needs from it is its class.
    #[must_use]
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::DailyLimitExceeded => Some("dailyLimitExceeded"),
            Self::RateLimitExceeded => Some("rateLimitExceeded"),
            Self::UserRateLimitExceeded => Some("userRateLimitExceeded"),
            Self::DomainPolicy => Some("domainPolicy"),
            Self::AuthError => Some("authError"),
            Self::BackendError => Some("backendError"),
            Self::BadRequest => Some("badRequest"),
            Self::Unrecognised => None,
        }
    }

    /// Returns whether this reason needs a person rather than a retry.
    ///
    /// `DailyLimitExceeded` and `DomainPolicy` both do, and for different people: the first needs whoever owns
    /// the Cloud project, the second needs a Workspace administrator. Grouped here because both need *someone*,
    /// with the distinction left to [`Self::as_str`] so a diagnostic can say which.
    #[must_use]
    pub const fn needs_a_person(self) -> bool {
        matches!(self, Self::DailyLimitExceeded | Self::DomainPolicy)
    }
}

/// Classifies a Google error response.
///
/// `status` is the HTTP status and `reason` the parsed `errors[0].reason`. `retry_after_seconds` is the
/// provider's stated delay when the response carried one, which a 429 does.
///
/// # The two decisions worth reading
///
/// - **A 403's meaning comes from its `reason`, never from the status.** Four documented reasons share the
///   status and have three different remedies: back off (`rateLimitExceeded`, `userRateLimitExceeded`), get a
///   person (`dailyLimitExceeded`, `domainPolicy`), or reauthorize (`authError` arrives as 401). A classifier
///   switching on the status alone would retry an administrator's decision.
/// - **An unrecognised status becomes `Unknown`, whose guidance is `Reconcile` and whose class refuses any
///   retry.** That is the fail-closed direction: a status this connector has no case for is one whose effect is
///   unknown, and `RetryClass::Unknown` already encodes that an unanswered question must not be read as a yes.
#[must_use]
pub fn classify(
    status: u16,
    reason: GmailErrorReason,
    retry_after_seconds: Option<u32>,
    provider_request_id: Option<ProviderRequestId>,
) -> RetryDecision {
    let (class, guidance) = match status {
        // A refusal that was not carried out, so there is nothing to reconcile and nothing to retry. A 404 is
        // included because a missing resource is not an invitation to try again.
        400 | 404 => (RetryClass::Permanent, RetryGuidance::DoNotRetry),
        401 => (RetryClass::Authentication, RetryGuidance::Reauthenticate),
        403 => match reason {
            GmailErrorReason::RateLimitExceeded | GmailErrorReason::UserRateLimitExceeded => (
                RetryClass::Throttled,
                RetryGuidance::BackoffSeconds(GOOGLE_RETRY_FLOOR_SECONDS),
            ),
            // `authError` as a 403 is not a documented combination — it is documented as a 401 — but a 403
            // whose reason says the credential is the problem must not be retried as if it were transient.
            GmailErrorReason::AuthError => {
                (RetryClass::Authentication, RetryGuidance::Reauthenticate)
            }
            GmailErrorReason::BackendError => (
                RetryClass::ProviderFault,
                RetryGuidance::BackoffSeconds(GOOGLE_RETRY_FLOOR_SECONDS),
            ),
            // The four permanent reasons, and they are permanent for two different reasons that are worth
            // naming. `dailyLimitExceeded` is permanent because the daily threshold **cannot be raised** and
            // the documented remedy is to change the Cloud project's quota — not something a retry does.
            // `domainPolicy` is permanent because it is an administrator's decision ("the domain
            // administrators have disabled Gmail apps"), and no amount of retrying changes a person's mind.
            // `badRequest` failed on its own terms. And `Unrecognised` joins them deliberately: a 403 is a
            // refusal with **no effect**, so the honest reading of an unknown 403 reason is "refused, do not
            // retry" rather than `Unknown`/`Reconcile`, which would send a caller to establish whether an
            // effect happened when the status already says it did not. A new *throttling* reason appearing as
            // a 403 is the cost of that choice, and it fails in the direction that cannot cause a second
            // effect.
            GmailErrorReason::DailyLimitExceeded
            | GmailErrorReason::DomainPolicy
            | GmailErrorReason::BadRequest
            | GmailErrorReason::Unrecognised => (RetryClass::Permanent, RetryGuidance::DoNotRetry),
        },
        // A 429 conflates three documented causes — the sending limit, the bandwidth limit, and per-user
        // concurrency — and the remedy for all three is the same: wait the stated time. The response carries a
        // retry time for the first two, so a stated delay is honoured and its absence falls back to the floor.
        429 => (
            RetryClass::Throttled,
            match retry_after_seconds {
                Some(seconds) => RetryGuidance::RetryAfterSeconds(seconds),
                None => RetryGuidance::BackoffSeconds(GOOGLE_RETRY_FLOOR_SECONDS),
            },
        ),
        500 | 502 | 503 | 504 => (
            RetryClass::ProviderFault,
            RetryGuidance::BackoffSeconds(GOOGLE_RETRY_FLOOR_SECONDS),
        ),
        // Every remaining status is unclassified, including 5xx codes Google does not document. `Unknown`
        // rather than a guess, and its guidance is to reconcile — never to retry.
        _ => (RetryClass::Unknown, RetryGuidance::Reconcile),
    };
    RetryDecision {
        class,
        guidance,
        provider_request_id,
    }
}

/// Why a page token was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PageTokenError {
    /// The token is empty, oversized, or holds a control character.
    #[error("the page token is unusable: {reason}")]
    Token {
        /// What is wrong.
        reason: &'static str,
    },
}

/// Returns the next page's token, validated.
///
/// # Errors
///
/// Returns [`PageTokenError::Token`] when the token is empty, longer than [`MAX_PAGE_TOKEN_CHARS`], or holds a
/// control character. The control-character refusal is not cosmetic: this value becomes a query parameter and
/// appears in a log line, and a newline in either forges a record.
pub fn next_page(next_page_token: Option<&str>) -> Result<Option<String>, PageTokenError> {
    let Some(token) = next_page_token else {
        return Ok(None);
    };
    if token.is_empty() {
        return Err(PageTokenError::Token {
            reason: "an empty page token is not the same as an absent one; absent means the last page",
        });
    }
    if token.chars().count() > MAX_PAGE_TOKEN_CHARS {
        return Err(PageTokenError::Token {
            reason: "a page token may be at most 4096 characters",
        });
    }
    if token.chars().any(char::is_control) {
        return Err(PageTokenError::Token {
            reason: "a page token may not hold a control character, because it becomes a request parameter \
                     and a log field",
        });
    }
    Ok(Some(token.to_owned()))
}

/// What a cursor advance concluded about the provider's history.
///
/// What a provider's answer to an incremental read **signals** about the cursor.
///
/// # Why the caller supplies this rather than the status
///
/// Gmail's staleness signal cannot be read from a status alone: a `startHistoryId` outside the retained range
/// returns an ordinary **`404`**, and the error guide documents `404` as "the requested resource couldn't be
/// found" with **no `reason` code** that distinguishes a pruned history from an absent mailbox. So "this status
/// means the cursor is unusable" is an inference that depends on **which method was called**, and only the
/// caller knows that. Requiring the signal makes the inference a caller's explicit act instead of a comparison
/// hidden inside a function that never saw the request.
///
/// **This type is the fix for a real gap**: `advance_gmail_history` used to take only `next_history_id`, so
/// [`SyncAdvance::HistoryPruned`] — the documented remedy for a stale cursor — was **unreachable**, and
/// [`SyncAdvance::TokenInvalidated`] was unreachable for Calendar the same way.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyncSignal {
    /// The read succeeded, and the response carried this position when it had one.
    ///
    /// `None` means the mailbox was unchanged since the cursor, which is an ordinary outcome rather than a
    /// failure — a caller that had to invent a position for it would store a guess.
    Advanced {
        /// The provider's new position, when it stated one.
        history_id: Option<String>,
    },
    /// The provider's answer means the cursor can no longer be used.
    ///
    /// One variant for both providers because the *signal* is one thing — "this position is dead" — while the
    /// reason differs: Gmail prunes history, Calendar invalidates a token. Each `advance_*` function names its
    /// own reason, so the distinction is not lost.
    CursorUnusable,
    /// The provider refused for a reason that is neither of the above, with the caller's classification.
    ///
    /// Carried rather than swallowed, so a caller can report *why* rather than an unexplained failure to
    /// advance. The caller classifies because it is the component that has the response body.
    Refused(RetryDecision),
}

/// **Not `Copy`**, unlike the enums around it, and the reason is [`Self::Refused`]: it carries a
/// [`RetryDecision`], which holds an optional provider request id — a `String`. Losing `Copy` is the honest
/// cost of carrying *why* rather than a bare verdict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SyncAdvance {
    /// The provider returned a new position; the walk may continue.
    Advanced,
    /// Gmail's history was pruned past the cursor's position.
    ///
    /// **The finding this module was written around.** A `startHistoryId` outside the retained range returns
    /// an ordinary **HTTP 404**, and the documented remedy is a full sync. `404` is also what an absent
    /// resource returns, so a connector that mapped `404` to "not found" for *every* method would report a
    /// pruned history as a missing account — two opposite remedies from one status.
    HistoryPruned,
    /// Calendar refused the sync token as invalid, with **410 Gone**.
    ///
    /// No ambiguity here: Google's sync guide says a `410` "should trigger a full wipe of the client's store
    /// and a new full sync", and `400` (a disallowed query restriction) is a different failure with a
    /// different remedy.
    TokenInvalidated,
    /// The provider refused the request for a reason that is neither of the above.
    ///
    /// Carried with the classified decision rather than swallowed, so a caller can report *why* rather than
    /// an unexplained failure to advance.
    Refused(RetryDecision),
}

/// The outcome of advancing a cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorOutcome {
    /// What happened.
    pub advance: SyncAdvance,
    /// The cursor to store, which is the **previous** one unless the advance succeeded.
    ///
    /// Held alongside the verdict so a caller cannot store a new position after a refusal. `P5-001`'s reason
    /// for a `SyncCursor` carrying its own account applies here: two values that must agree, and the honest
    /// thing is for one value to hold both.
    pub cursor: Option<SyncCursor>,
}

/// Advances a Gmail history cursor from what the provider's answer **signalled**.
///
/// A read that succeeded advances the cursor. [`SyncSignal::CursorUnusable`] — which the caller produces from a
/// **`404` on `users.history.list`**, the status the Gmail sync guide documents as a `startHistoryId` outside
/// the retained window — requires a full sync, so the previous cursor is **not** carried forward: a caller must
/// not resume from a position the provider has already rejected.
///
/// **The caller must only produce `CursorUnusable` for `users.history.list`.** A `404` from
/// `users.messages.get` means the message does not exist, and mapping that to a resync would discard a whole
/// sync over one missing message. That is why the signal is a parameter: only the caller knows which method it
/// called, and the inference from a `404` to "history pruned" is exactly the part that depends on it.
///
/// # Why a `404` may be read as pruned even though it is ambiguous
///
/// Google documents **no `reason` code** for a `404` on this call, so "history pruned" cannot be *proved* from
/// the response — a `404` is equally what an absent mailbox returns. The two readings **converge by
/// consequence**, and that is what makes the choice safe rather than merely convenient: if the history was
/// pruned, the full sync is the documented remedy; if the mailbox is genuinely gone or the account is
/// disconnected, the full sync's own first call fails and *that* surfaces the truth. The wrong reading is
/// therefore **self-correcting**, and its worst case is one extra `messages.list` call rather than a store that
/// resumes from a dead position and reports itself in sync.
///
/// # Errors
///
/// Returns [`CursorError`] when the new cursor is unusable — an empty or oversized `historyId`, which is
/// provider-issued text that becomes a request parameter.
pub fn advance_gmail_history(
    previous: &SyncCursor,
    signal: &SyncSignal,
    account: &AccountReference,
    connector_version: &ConnectorVersion,
    now: UtcTimestamp,
) -> Result<CursorOutcome, CursorError> {
    let history_id = match signal {
        SyncSignal::Advanced { history_id } => history_id.as_deref(),
        SyncSignal::CursorUnusable => {
            return Ok(CursorOutcome {
                advance: SyncAdvance::HistoryPruned,
                // No cursor. Carrying the old one forward would invite a caller to resume from the position the
                // provider just rejected, which is the defect this arm exists to prevent.
                cursor: None,
            });
        }
        SyncSignal::Refused(decision) => {
            return Ok(CursorOutcome {
                advance: SyncAdvance::Refused(decision.clone()),
                cursor: None,
            });
        }
    };
    let Some(history_id) = history_id else {
        // No new id means the mailbox is unchanged since the cursor. That is an ordinary outcome, not a
        // failure, and the previous cursor is returned unchanged rather than replaced by a guess.
        return Ok(CursorOutcome {
            advance: SyncAdvance::Advanced,
            cursor: Some(previous.clone()),
        });
    };
    let next = SyncCursor::new(
        SyncCursorKind::MonotonicMarker,
        Some(history_id.to_owned()),
        account.clone(),
        connector_version.as_str(),
        now,
    )?;
    // A monotonic marker that moves **backwards** is refused rather than stored. `historyId` increases, so a
    // smaller value means the response is for an earlier position than the cursor holds — a replayed or
    // stale response, or one belonging to another account's history. Storing it would silently re-walk
    // history the connector has already processed, which for a connector that acts on changes is a repeat.
    if let (Some(previous_token), Some(next_token)) = (previous.token(), next.token())
        && let (Ok(previous_id), Ok(next_id)) =
            (previous_token.parse::<u64>(), next_token.parse::<u64>())
        && next_id < previous_id
    {
        return Err(CursorError::Shape {
            reason: "a `historyId` may not move backwards; a smaller value is a stale or foreign \
                     response, and storing it would re-walk history the connector has processed",
        });
    }
    Ok(CursorOutcome {
        advance: SyncAdvance::Advanced,
        cursor: Some(next),
    })
}

/// Advances a Calendar sync cursor from what the provider's answer **signalled**.
///
/// Calendar's staleness signal is unambiguous where Gmail's is not: **410 Gone** invalidates the token and
/// requires a full wipe, while **400** is a disallowed query restriction — a caller's mistake, not a stale
/// token — so it is refused through [`SyncSignal::Refused`] rather than triggering a resync.
///
/// # Errors
///
/// Returns [`CursorError`] when the new token is unusable.
pub fn advance_calendar_sync(
    previous: &SyncCursor,
    signal: &SyncSignal,
    account: &AccountReference,
    connector_version: &ConnectorVersion,
    now: UtcTimestamp,
) -> Result<CursorOutcome, CursorError> {
    let token = match signal {
        SyncSignal::Advanced { history_id } => history_id.as_deref(),
        SyncSignal::CursorUnusable => {
            return Ok(CursorOutcome {
                advance: SyncAdvance::TokenInvalidated,
                cursor: None,
            });
        }
        SyncSignal::Refused(decision) => {
            return Ok(CursorOutcome {
                advance: SyncAdvance::Refused(decision.clone()),
                cursor: None,
            });
        }
    };
    let Some(token) = token else {
        return Ok(CursorOutcome {
            advance: SyncAdvance::Advanced,
            cursor: Some(previous.clone()),
        });
    };
    // `nextSyncToken` is opaque — nothing may be concluded from it, which is `SyncCursorKind::OpaqueToken`'s
    // whole content. So there is no ordering check here, unlike Gmail's monotonic marker: a comparison would
    // be inventing a property the provider never offered.
    let next = SyncCursor::new(
        SyncCursorKind::OpaqueToken,
        Some(token.to_owned()),
        account.clone(),
        connector_version.as_str(),
        now,
    )?;
    Ok(CursorOutcome {
        advance: SyncAdvance::Advanced,
        cursor: Some(next),
    })
}

/// Returns whether a Calendar incremental-sync status requires a full resync.
///
/// Separated from [`advance_calendar_sync`] because it answers a question about a **failure** while that
/// function answers one about a success, and a caller needs both: it will see the 410 in its own error path.
/// The result is what the caller passes as [`SyncSignal::CursorUnusable`], so the inference and the decision
/// stay adjacent rather than one being hidden inside the other.
#[must_use]
pub const fn calendar_status_requires_resync(status: u16) -> bool {
    status == 410
}

/// Returns whether a Gmail **`history.list`** status cannot prove the cursor is unusable.
///
/// # This reverses an earlier answer, and the reversal is the finding
///
/// An earlier version of this function was named `gmail_history_status_is_pruned` and returned
/// `status == 404`. That **asserted** the distinction Finding 2 in `docs/research/integrations/google.md`
/// says cannot be made: the sync guide documents a `404` for a `startHistoryId` outside the retained range,
/// while the error guide documents `404` as "the requested resource couldn't be found" with **no `reason`
/// code** — so the same status, with the same code, is what an absent mailbox returns. A predicate that says
/// "pruned" is claiming knowledge the response does not carry, and a caller reading it would believe the
/// connector had distinguished two cases it never distinguished.
///
/// So the predicate now answers what can actually be read: **"this status carries no distinguishing
/// information"**. A caller still resyncs on it — see [`advance_gmail_history`] for why that is safe — but it
/// does so through [`SyncSignal::CursorUnusable`], which is a deliberate act, rather than through a name that
/// implied the question was settled.
///
/// `404` is the one status where a resync may be warranted; `429` and the `5xx` family are retryable and a
/// resync would discard a working store over a transient failure.
#[must_use]
pub const fn gmail_history_status_cannot_prove_usable(status: u16) -> bool {
    status == 404
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
