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

use crate::google::transport::RetryAfter;
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

/// The most calls Gmail **accepts** in one batch request.
///
/// A **hard** limit, and the batch reference's own words are unambiguous: "You're limited to 100 calls in a
/// single batch request. If you must make more calls than that, use multiple batch requests." Exceeding it is
/// a refusal rather than a slowdown, so a caller must chunk against this figure.
pub const GMAIL_BATCH_HARD_LIMIT: u32 = 100;

/// The batch size Gmail **recommends**, because a batch is itself a rate-limit trigger.
///
/// The same page: "Larger batch sizes are likely to trigger rate limiting. We recommend sending batches of no
/// more than 50 requests." A **recommendation**, not a ceiling — nothing is refused for exceeding it, so a
/// caller that treats it as a limit is merely conservative, while one that treats [`GMAIL_BATCH_HARD_LIMIT`] as
/// a target risks throttling a whole batch.
///
/// # Why these are two constants rather than one
///
/// The two were **one constant of 50**, documented as "the largest batch Gmail accepts" — which is false, and
/// conflates a refusal with a slowdown. The distinction decides the failure mode: exceeding the hard limit
/// fails the request, exceeding the recommendation degrades throughput. A single figure cannot express both,
/// and naming it after the wrong one sends a caller to the wrong remedy — a batch of 60 is *permitted* and
/// merely unwise, while a batch of 101 is refused.
pub const GMAIL_BATCH_RECOMMENDED: u32 = 50;

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
pub struct GoogleErrorBody {
    /// The error object.
    pub error: GoogleErrorObject,
}

/// The `error` object of a Gmail error response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct GoogleErrorObject {
    /// The HTTP status, repeated in the body.
    pub code: u16,
    /// The machine-readable reasons.
    ///
    /// An array because the documented samples show one entry per error; `default` because a body without one
    /// is still a body, and a missing reason must degrade to "unclassified" rather than to a parse failure.
    #[serde(default)]
    pub errors: Vec<GoogleErrorEntry>,
}

/// One entry of a Gmail error's `errors` array.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct GoogleErrorEntry {
    /// The machine-readable reason, which is what a classifier switches on.
    ///
    /// Optional on purpose: the documentation's own samples do not all carry one, and an absent reason must
    /// leave the caller able to classify from the status rather than refusing to parse the body.
    #[serde(default)]
    pub reason: Option<String>,
}

impl GoogleErrorBody {
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
pub enum GoogleErrorReason {
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

impl GoogleErrorReason {
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

/// Which Google API a response came from.
///
/// # Why the classifier needs to be told, and why it could not infer it
///
/// This connector talks to **two** APIs, and Google documents them on **separate** error pages with
/// **different status sets**:
///
/// - The **Gmail** page's status summary lists `200`, `400`, `401`, `403`, `404`, `429` and the `5xx` family.
///   It has **no `410` subsection at all**.
/// - The **Calendar** page documents `410 Gone` in detail, as a **dead sync token** whose remedy is "wipe the
///   store and re-sync".
///
/// So the same status is a documented, definite state for one API and an unclassified one for the other. A
/// classifier that could not tell them apart had to answer for the **less** informative case, and it did: a
/// Calendar `410` reached a caller as `unknown`/"reconcile" — *"establish what happened before doing anything
/// else"* — when the provider had said exactly what happened.
///
/// # Why this is a parameter rather than two classifiers
///
/// Two tables would duplicate every arm that the APIs **do** share (`401`, the `5xx` family, and `403`'s
/// throttling reasons, which Calendar's page confirms behave the same: "`rateLimitExceeded` errors can return
/// either `403` or `429` error codes—currently they are functionally similar"). A duplicated table drifts, and
/// the phase has found that defect repeatedly. So there is **one** table and the API is an input, exactly as
/// `status` and `reason` are.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GoogleApi {
    /// Gmail `v1`.
    Gmail,
    /// Calendar `v3`.
    Calendar,
}

/// Classifies a Google error response.
///
/// `api` is which of the two APIs answered, because their documented status sets differ; `status` is the HTTP
/// status and `reason` the parsed `errors[0].reason`. `retry_after` is the provider's stated delay **in the form
/// it was stated in**, which a 429 does.
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
/// - **A `410` is a known state for Calendar and an unclassified one for Gmail.** See [`GoogleApi`]; the
///   per-API arm is the reason this function takes an API at all.
/// - **A stated delay that cannot be read is not the same as no stated delay.** `retry_after` distinguishes
///   *absent* (`None`) from *stated but unreadable* ([`RetryAfter::NotSeconds`]), and the `429` arm keeps them
///   apart (`ADR-0076`).
#[must_use]
pub fn classify(
    api: GoogleApi,
    status: u16,
    reason: GoogleErrorReason,
    retry_after: Option<RetryAfter>,
    provider_request_id: Option<ProviderRequestId>,
) -> RetryDecision {
    let (class, guidance) = match (api, status) {
        // **Two different routes to one verdict, so this is one arm.** The verdict is
        // `Permanent`/`DoNotRetry`; the two routes reach it for unrelated reasons, and the reasons are kept
        // side by side because a reader deciding whether either route applies needs both.
        //
        // **Route one — Calendar's `410 Gone` — is the reason this function takes an API at all.** The
        // Calendar error page documents `410` as a dead sync token — "wipe the store and re-sync" — while the
        // Gmail page has **no `410` subsection at all**, so the same status is a known remedy for one API and
        // an unclassified one for the other. Before this pattern existed, a Calendar `410` fell to the
        // catch-all below and reached a caller as `unknown`/"reconcile", which told it to *establish what
        // happened* when the provider had already said: the cursor is dead, resync.
        //
        // The class is `Permanent` because a resync is not a *retry of this request* — the request cannot
        // succeed with this token however often it is sent. `DoNotRetry` is therefore correct here and does
        // **not** contradict the remedy: the caller's next action is a fresh full sync, which is a different
        // request. Reading the guidance as "give up on the sync" would be the mistake, so the distinction is
        // spelled out rather than implied.
        //
        // **Route two — a `400` or `404` — is a refusal that was not carried out**, so there is nothing to
        // reconcile and nothing to retry. A 404 is included because a missing resource is not an invitation to
        // try again.
        //
        // **A documented divergence, kept deliberately.** Calendar's error page suggests "use exponential
        // backoff" for a `404`, while Gmail's summary states no action. The crate follows neither literally:
        // it keeps `DoNotRetry`, because Calendar's own two documented causes are "the requested resource …
        // has never existed" and "accessing a calendar that the user can not access", and neither is repaired
        // by sending the same request again — a retry would fail identically until it gave up. The divergence
        // is asserted in a test and recorded in the research record rather than silently resolved, so the next
        // reader meets the decision instead of the discrepancy (`ADR-0082`).
        (GoogleApi::Calendar, 400 | 404 | 410) | (_, 400 | 404) => {
            (RetryClass::Permanent, RetryGuidance::DoNotRetry)
        }
        (_, 401) => (RetryClass::Authentication, RetryGuidance::Reauthenticate),
        (_, 403) => match reason {
            GoogleErrorReason::RateLimitExceeded | GoogleErrorReason::UserRateLimitExceeded => (
                RetryClass::Throttled,
                RetryGuidance::BackoffSeconds(GOOGLE_RETRY_FLOOR_SECONDS),
            ),
            // `authError` as a 403 is not a documented combination — it is documented as a 401 — but a 403
            // whose reason says the credential is the problem must not be retried as if it were transient.
            GoogleErrorReason::AuthError => {
                (RetryClass::Authentication, RetryGuidance::Reauthenticate)
            }
            GoogleErrorReason::BackendError => (
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
            GoogleErrorReason::DailyLimitExceeded
            | GoogleErrorReason::DomainPolicy
            | GoogleErrorReason::BadRequest
            | GoogleErrorReason::Unrecognised => (RetryClass::Permanent, RetryGuidance::DoNotRetry),
        },
        // A 429 conflates three documented causes — the sending limit, the bandwidth limit, and per-user
        // concurrency — and the remedy for all three is the same: wait the stated time. The response carries a
        // retry time for the first two, so a stated delay is honoured and its absence falls back to the floor.
        //
        // **The three cases are kept apart.** A `delay-seconds` value is honoured directly; the date form is a
        // stated delay this client cannot convert without a clock, so it falls back to the floor **but says so**
        // (`BackoffAfterUnreadableDelay` rather than `BackoffSeconds`); and only a genuinely absent header falls
        // back silently. Collapsing the middle case into the last would tell an operator the provider stated
        // nothing when it stated a time — the direction that reads a stated wait as a missing one (`ADR-0076`).
        //
        // **A stated delay above `MAX_RETRY_AFTER_SECONDS` becomes `DeferSeconds`, not a clamp.** Google
        // documents that a daily-limit 429 "might result in these errors for multiple hours", so this is a real
        // response rather than a hypothetical: the work is deferred, because clamping would retry sooner than
        // the provider asked and honouring it in the loop would hold a worker for the whole window
        // (`ADR-0077`).
        (_, 429) => (
            RetryClass::Throttled,
            match retry_after {
                Some(RetryAfter::Seconds(seconds)) => RetryGuidance::for_stated_delay(seconds),
                Some(RetryAfter::NotSeconds) => {
                    RetryGuidance::BackoffAfterUnreadableDelay(GOOGLE_RETRY_FLOOR_SECONDS)
                }
                None => RetryGuidance::BackoffSeconds(GOOGLE_RETRY_FLOOR_SECONDS),
            },
        ),
        (_, 500 | 502 | 503 | 504) => (
            RetryClass::ProviderFault,
            RetryGuidance::BackoffSeconds(GOOGLE_RETRY_FLOOR_SECONDS),
        ),
        // Every remaining status is unclassified, including 5xx codes Google does not document and a `410`
        // from an API that does not publish one. `Unknown` rather than a guess, and its guidance is to
        // reconcile — never to retry.
        (_, _) => (RetryClass::Unknown, RetryGuidance::Reconcile),
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

/// Why a call count could not be planned into batches.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum BatchPlanError {
    /// No batch size may be zero, because a zero-sized batch would never make progress.
    #[error("a batch size must be at least one call")]
    EmptyBatch,
    /// A batch size above the hard limit would be refused by the provider.
    #[error("a batch of {requested} calls exceeds Gmail's hard limit of {maximum}")]
    AboveHardLimit {
        /// The requested size.
        requested: u32,
        /// [`GMAIL_BATCH_HARD_LIMIT`].
        maximum: u32,
    },
}

/// How a set of calls divides into batch requests.
///
/// # Why this is a type rather than two `u32`s at a call site
///
/// The research record's Verification Plan asks for "a full-sync budget test that asserts batching", and the
/// two facts a caller needs are different in kind: **how many requests to send** (a division against a hard
/// limit) and **whether the size is the recommended one** (a comparison against a recommendation that is not
/// enforced). Returning them together means a caller cannot take the request count and lose the fact that the
/// size it chose invites throttling — the arrangement `RetryDecision` already uses for a class and its
/// guidance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatchPlan {
    /// The size each batch request may carry.
    pub batch_size: u32,
    /// How many batch requests the calls divide into.
    ///
    /// Rounded **up**, because the last batch may be partial and a caller that under-counted would drop it:
    /// 101 calls at 50 per batch is three requests, not two.
    pub requests: u32,
    /// The size of the final, possibly partial batch — zero when the calls divide evenly.
    ///
    /// Carried rather than left to arithmetic because the partial batch is the one a naive planner drops, and
    /// because `calls % batch_size` recomputed at a call site is a second answer to a question this type has
    /// already settled.
    pub final_batch_size: u32,
}

impl BatchPlan {
    /// Returns whether the size is the provider's **recommendation** rather than the hard limit.
    ///
    /// The predicate that keeps the two figures distinct at a call site: `true` means the plan is within the
    /// size Google recommends when a batch is itself a rate-limit trigger; `false` does not mean the plan is
    /// refused — a size between the recommendation and [`GMAIL_BATCH_HARD_LIMIT`] is permitted and merely
    /// invites throttling.
    #[must_use]
    pub const fn is_within_recommendation(self) -> bool {
        self.batch_size <= GMAIL_BATCH_RECOMMENDED
    }
}

/// Divides a number of calls into batch requests of a given size.
///
/// The two bounds are checked against the **documented pair**, and the direction of each refusal is the point:
/// a size above [`GMAIL_BATCH_HARD_LIMIT`] is refused because the provider would reject the request, while a
/// size above [`GMAIL_BATCH_RECOMMENDED`] is **allowed** and reported instead — throttling is a risk, not a
/// refusal, and refusing it would be stricter than the provider.
///
/// # Errors
///
/// Returns [`BatchPlanError::EmptyBatch`] for a zero size, and [`BatchPlanError::AboveHardLimit`] for a size
/// above [`GMAIL_BATCH_HARD_LIMIT`]. A zero **call count** is not an error: nothing to send is no requests,
/// with a zero-sized final batch.
///
/// # Errors
///
/// See the two variants above.
pub const fn batch_plan(calls: u32, batch_size: u32) -> Result<BatchPlan, BatchPlanError> {
    if batch_size == 0 {
        return Err(BatchPlanError::EmptyBatch);
    }
    if batch_size > GMAIL_BATCH_HARD_LIMIT {
        return Err(BatchPlanError::AboveHardLimit {
            requested: batch_size,
            maximum: GMAIL_BATCH_HARD_LIMIT,
        });
    }
    // `calls.div_ceil(batch_size)` is the rounded-up division; it is not const-stable on the pinned toolchain,
    // so the arithmetic is spelled out. Zero calls makes this zero rather than one, which is right: there is
    // nothing to send.
    let remainder = calls % batch_size;
    let requests = calls / batch_size + if remainder == 0 { 0 } else { 1 };
    Ok(BatchPlan {
        batch_size,
        requests,
        final_batch_size: remainder,
    })
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
/// **410 Gone** invalidates the token and requires a full wipe, while **400** is a disallowed query
/// restriction — a caller's mistake, not a stale token — so it is refused through [`SyncSignal::Refused`]
/// rather than triggering a resync.
///
/// # The status alone is not the whole answer, and this function cannot see the rest
///
/// A `410` has **three** documented causes and only two of them resync — see [`CalendarGoneReason`]. This
/// function takes the caller's [`SyncSignal`], so the caller does the reading: one that consults
/// [`CalendarGoneReason::requires_resync`] will not send [`SyncSignal::CursorUnusable`] for a `deleted`, which
/// is the difference between a resync and a discarded store. The shape is the same one `ADR-0066` established
/// for Gmail — the **inference** belongs to the caller and the **decision** to this function — because only the
/// caller knows which method it called and what the body said.
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

/// What a Calendar **`410 Gone`** means for the sync store.
///
/// # Why a status is not enough, on the call where it matters most
///
/// The Calendar errors page publishes **three** distinct bodies for `410 Gone`, and they do not share a
/// remedy:
///
/// | `reason` | Message | Suggested action |
/// | --- | --- | --- |
/// | `fullSyncRequired` | "Sync token is no longer valid, a full sync is required." | wipe the store and re-sync |
/// | `updatedMinTooLongAgo` | "The requested minimum modification time lies too far in the past." | wipe the store and re-sync |
/// | `deleted` | "Resource has been deleted" | **"no further action is necessary"** |
///
/// So a connector that reads the **status alone** wipes the whole sync store when a user deletes one event —
/// and a resync is the expensive mistake, not the cheap one. The research record had asserted that "Calendar's
/// `410` has no such ambiguity" (contrasting it with Gmail's unclassifiable `404`); the errors page shows the
/// status alone does not choose a remedy, and the corrected statement is that a `410` is unambiguous **once
/// the `reason` is read**. Same shape as the Gmail `403`: one status, several remedies, and the reason decides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalendarGoneReason {
    /// The sync token or `updatedMin` is no longer valid, so the store must be wiped and resynced.
    ///
    /// Covers both documented sync-token causes (`fullSyncRequired`, `updatedMinTooLongAgo`) because they
    /// share a remedy, and the distinction between them is a detail for a diagnostic rather than for the
    /// decision — the same reasoning `GoogleErrorReason::needs_a_person` records for its own grouping.
    FullSyncRequired,
    /// The resource was already deleted, so **there is nothing to resync**.
    ///
    /// The variant that stops a deleted event from costing a whole store.
    ResourceAlreadyDeleted,
    /// A `410` whose `reason` this crate does not recognise.
    ///
    /// **Resyncs**, and that is the fail-safe direction for a *cursor*: an unrecognised `410` might be a new
    /// sync-token cause, and the failure mode of resyncing needlessly is a slower next sync, while the failure
    /// mode of *not* resyncing a genuinely dead token is a store that never syncs again and never says so. The
    /// record's own rule for `Unknown` retry classes is the opposite because it protects a **non-idempotent
    /// effect**; here the protected thing is a store's liveness.
    Unrecognised,
}

impl CalendarGoneReason {
    /// Parses the `reason` from a Calendar `410` body.
    ///
    /// `None` — a body with no reason at all — is [`Self::Unrecognised`], because an absent reason is not a
    /// statement that the token is valid.
    #[must_use]
    pub fn parse(reason: Option<&str>) -> Self {
        match reason {
            Some("fullSyncRequired" | "updatedMinTooLongAgo") => Self::FullSyncRequired,
            Some("deleted") => Self::ResourceAlreadyDeleted,
            Some(_) | None => Self::Unrecognised,
        }
    }

    /// Returns whether the sync store must be wiped and resynced.
    ///
    /// The predicate the caller acts on. `false` for [`Self::ResourceAlreadyDeleted`] — the case the status
    /// alone gets wrong, and the reason this type exists.
    #[must_use]
    pub const fn requires_resync(self) -> bool {
        matches!(self, Self::FullSyncRequired | Self::Unrecognised)
    }
}

/// Returns whether a Calendar incremental-sync status requires a full resync.
///
/// Separated from [`advance_calendar_sync`] because it answers a question about a **failure** while that
/// function answers one about a success, and a caller needs both: it will see the 410 in its own error path.
/// The result is what the caller passes as [`SyncSignal::CursorUnusable`], so the inference and the decision
/// stay adjacent rather than one being hidden inside the other.
///
/// # Use [`CalendarGoneReason`] where the body is readable
///
/// This predicate is the **status-only** answer, and it is deliberately permissive: every `410` resyncs, so a
/// caller that has only a status — or a body it could not parse — still recovers a dead token. A caller that
/// **can** read the reason must use [`CalendarGoneReason::requires_resync`] instead, because one of the three
/// documented `410` causes (`deleted`) says "no further action is necessary" and a resync there discards a
/// working store over a single deleted event. The two are not redundant: this one cannot be made stricter
/// without losing the unparseable-body case, and that one cannot be made more permissive without losing the
/// distinction.
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

/// Turns a `history.list` answer into the signal [`advance_gmail_history`] consumes.
///
/// **This is what makes [`SyncAdvance::HistoryPruned`] reachable from a real response.** `ADR-0066` made the
/// signal a parameter so that the inference from a `404` to "history pruned" would be the caller's explicit
/// act — but the signal then had **no producer**, because there was no `history.list` request to obtain a
/// status from. A parameter whose only producer is a test fixture is the unreachable-remedy defect one layer
/// up, so this function is that producer, placed beside the predicate that answers half its question.
///
/// # The three outcomes
///
/// - **`404`** — the cursor is unusable. The response cannot distinguish pruned history from an absent
///   mailbox, and both causes begin with a full sync, so the wrong reading is self-correcting. See
///   [`gmail_history_status_cannot_prove_usable`].
/// - **`200`** — the read succeeded, so the mailbox's new `historyId` (when the response carried one)
///   advances the cursor. A `200` with no id means the mailbox was unchanged, which
///   [`advance_gmail_history`] handles by keeping the previous cursor rather than inventing a position.
/// - **anything else** — carried with the caller's classification rather than swallowed.
///
/// # What is deliberately NOT a dead cursor
///
/// A `429` or a `5xx` is **retryable**, not a dead cursor: a resync on a transient failure discards a working
/// store, which is the opposite mistake and a far more expensive one. The predicate is true for `404` alone,
/// and the ordering here puts that predicate first so the retryable family can never fall into it.
#[must_use]
pub fn gmail_history_signal(
    status: u16,
    history_id: Option<&str>,
    refusal: RetryDecision,
) -> SyncSignal {
    if gmail_history_status_cannot_prove_usable(status) {
        return SyncSignal::CursorUnusable;
    }
    if status == 200 {
        return SyncSignal::Advanced {
            history_id: history_id.map(str::to_owned),
        };
    }
    SyncSignal::Refused(refusal)
}

/// Turns a Calendar `events.list` answer into the signal [`advance_calendar_sync`] consumes.
///
/// The producer that makes [`SyncAdvance::TokenInvalidated`] reachable **for the right reason**. A caller that
/// used [`calendar_status_requires_resync`] alone would send [`SyncSignal::CursorUnusable`] for every `410`
/// including a `deleted` — which is a status the page says needs **no action** — so this function reads the
/// reason where the caller can supply one and falls back to the status-only answer where it cannot.
///
/// # The three outcomes
///
/// - **`410` whose reason is a sync-token cause, or unreadable** — the cursor is unusable; wipe and resync.
///   The unreadable case is included deliberately and its direction is argued on
///   [`CalendarGoneReason::Unrecognised`].
/// - **`410` whose reason is `deleted`** — the cursor is **fine**. The event is simply gone, so this is a
///   refusal for the *call* rather than a resync, and the store survives.
/// - **`200`** — the read succeeded, so the provider's `nextSyncToken` (when the response carried one)
///   advances the cursor; a `200` with no token means nothing changed, which [`advance_calendar_sync`] handles
///   by keeping the previous cursor.
/// - **anything else** — carried with the caller's classification rather than swallowed. A `400` in particular
///   arrives here rather than being read as staleness, which is the distinction the sync guide draws.
#[must_use]
pub fn calendar_signal(
    status: u16,
    next_sync_token: Option<&str>,
    gone_reason: Option<&str>,
    refusal: RetryDecision,
) -> SyncSignal {
    if status == 410 {
        return match CalendarGoneReason::parse(gone_reason) {
            reason if reason.requires_resync() => SyncSignal::CursorUnusable,
            // `ResourceAlreadyDeleted` reaches here: no resync, and the call is refused rather than pretended
            // to have succeeded — a delete of an already-deleted event did not do what was asked, even though
            // nothing needs repairing.
            _ => SyncSignal::Refused(refusal),
        };
    }
    if status == 200 {
        return SyncSignal::Advanced {
            history_id: next_sync_token.map(str::to_owned),
        };
    }
    SyncSignal::Refused(refusal)
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
