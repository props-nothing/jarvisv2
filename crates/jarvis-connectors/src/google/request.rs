//! Google request construction — the half of the client that needs no socket.
//!
//! # What this module is for
//!
//! A request is a *value* here: a method, a URL, and an ordered list of query parameters. Nothing sends it.
//! That is deliberate and it is the same split [`crate::google::client`] makes — the decisions are testable
//! without I/O, and the binding that performs I/O stays a separate, later step. `jarvis-models` reaches the
//! same shape with an async `Transport` trait, which becomes necessary only when something must actually
//! perform the request.
//!
//! # The two rules this module exists to enforce
//!
//! **1. A query value is percent-encoded, and that is a security control rather than tidiness.** A Gmail search
//! query is model-chosen text. Interpolated raw into `?q=…` it can contain `&`, `#`, or `=`, and the request
//! that leaves is a *different* request than the one intended: `is:unread&maxResults=999` turns a bounded
//! search into an unbounded one, and `#` truncates everything after it. So every value goes through
//! [`percent_encode`], and the tests assert the specific characters that would change the request.
//!
//! **2. The access token never appears in a URL.** Google's own documentation offers `?access_token=` as an
//! alternative to the header and adds that "query strings tend to be visible in server logs" — so the
//! documented preference is the header, and this type has **no field a token could go in**. That absence is
//! the control, in the same spirit as `jarvis_core::SecretRef` having no `value` field: a URL is the one part
//! of a request that ends up in logs, proxies, and error messages, so making it unable to hold a credential is
//! stronger than a rule about not putting one there.
//!
//! # What is not verified
//!
//! **No request has been sent.** The URLs are built from the API bases and the endpoints in
//! `docs/research/integrations/google.md`, and the tests assert the *shape* this module produces. Whether
//! Google accepts that shape is established only by the live smoke test that does not exist yet.

use std::fmt;

use serde::Deserialize;

use crate::google::CONNECTOR_ID;
use crate::google::client::{self, CALENDAR_API_BASE, GMAIL_API_BASE, GMAIL_MAX_RESULTS_CAP};

/// The largest `maxResults` Calendar accepts on an events list.
///
/// Larger than Gmail's 500 because the resource is different: an event is a small object and a calendar page
/// is a date range rather than a mailbox. Recorded as a per-API bound rather than shared, because using one
/// number for both APIs would be a bound that is wrong for one of them.
pub const CALENDAR_MAX_RESULTS_CAP: u32 = 2_500;

/// The longest a search query may be, in characters.
///
/// Bounded because the query reaches a provider and a log line; Gmail's own syntax has no published limit, so
/// this is **a JARVIS bound** rather than a provider figure — the distinction
/// [`crate::ratelimit::RateLimitEvidence`] exists to keep, and the reason the constant's doc says so.
pub const MAX_QUERY_CHARS: usize = 512;

/// The longest a message, thread, or calendar identifier may be, in characters.
///
/// A JARVIS bound for the same reason: the identifiers Google issues are far shorter, and a bound is what
/// stops a model-supplied string from becoming an unbounded URL.
pub const MAX_RESOURCE_ID_CHARS: usize = 256;

/// The longest a Calendar time bound may be, in characters.
///
/// **A JARVIS bound and a tight one, not [`MAX_QUERY_CHARS`].** An RFC 3339 instant is about 25 characters and
/// at most about 30 with fractional seconds and an offset, so a bound of 64 admits every valid instant with
/// room to spare while still refusing a string that is obviously not a timestamp. A time bound is not a search
/// query: it was previously checked against the query's 512-character limit and refused with a message about
/// a query, which is both the wrong noun and the wrong field name (`ADR-0086`).
pub const MAX_TIME_BOUND_CHARS: usize = 64;

/// The longest a Cloud Pub/Sub topic name may be, in characters.
///
/// A JARVIS bound, like [`MAX_RESOURCE_ID_CHARS`]: a fully qualified name is
/// `projects/{project}/topics/{topic}`, where Google caps a project id at 30 characters and a topic name at
/// 255, so the documented maximum is under 300 and this bound admits it with room to refuse an unbounded
/// model-supplied string.
pub const MAX_TOPIC_NAME_CHARS: usize = 512;

/// The most label identifiers a watch may be scoped to.
///
/// A JARVIS bound. Gmail has no published cap on `labelIds` in this request, and a label's own id is short, so
/// the purpose is to refuse a list whose size is really an attempt at an unbounded body rather than to model
/// a provider figure.
pub const MAX_WATCH_LABEL_IDS: usize = 64;

/// The `Accept` header value both APIs expect for a JSON body.
pub const JSON_ACCEPT: &str = "application/json";

/// Why a request could not be constructed.
///
/// Every variant names a **field of the caller's argument**, because the caller is either a model choosing
/// arguments or the operation layer above. A single "invalid request" variant would send a reader looking for
/// a transport fault in a value that was refused before any transport existed.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RequestError {
    /// An identifier is empty, oversized, or holds a control character.
    #[error("the `{field}` argument is unusable: {reason}")]
    Argument {
        /// Which argument.
        field: &'static str,
        /// What is wrong.
        reason: &'static str,
    },
    /// A `max_results` value is outside what the API accepts.
    #[error("`max_results` must be 1 to {maximum} for this API, not {requested}")]
    MaxResults {
        /// What was asked for.
        requested: u32,
        /// The API's documented cap.
        maximum: u32,
    },
    /// Two arguments were supplied that the provider **refuses together**.
    ///
    /// Distinct from [`Self::Argument`], which is about one value being malformed. Here both values may be
    /// perfectly valid and the **combination** is what the provider rejects — a class of mistake that a model,
    /// reading an input schema which advertises both fields, has no way to anticipate.
    #[error("`{field}` cannot be combined with `{other}`: {reason}")]
    DisallowedCombination {
        /// The argument that is fine on its own.
        field: &'static str,
        /// The argument it cannot be sent with.
        other: &'static str,
        /// Why the provider refuses the pair.
        reason: &'static str,
    },
    /// A page token is unusable.
    #[error(transparent)]
    PageToken(#[from] client::PageTokenError),
    /// A deprecated field was supplied, or an argument was sent that the provider would **silently ignore**.
    ///
    /// Distinct from [`Self::Argument`] and [`Self::DisallowedCombination`], and the difference is the failure
    /// mode: a disallowed combination is a `400` the caller sees, while an ignored argument is a request that
    /// **succeeds and does something else**. That is the class this variant exists for — nothing downstream
    /// reports it, so the only place it can be caught is before the request is sent.
    #[error("`{field}` would be ignored by the provider: {reason}")]
    Ignored {
        /// Which argument.
        field: &'static str,
        /// Why the provider ignores it, and what to send instead.
        reason: &'static str,
    },
}

/// A request to perform: a method, a URL, and ordered query parameters.
///
/// **There is no field for a credential**, and that absence is documented in the module. There is also no
/// body: every declared operation is a `GET`, so a request type that could carry a body would be a shape
/// nothing uses — and the first caller to put arguments in a body would be writing a request Google rejects
/// for a `GET` rather than one this module refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpRequest {
    method: &'static str,
    url: String,
    query: Vec<(String, String)>,
    accept: &'static str,
}

impl HttpRequest {
    /// Returns the HTTP method.
    #[must_use]
    pub const fn method(&self) -> &'static str {
        self.method
    }

    /// Returns the absolute URL, **without** the query string.
    ///
    /// Split from the parameters so a log line or a diagnostic can render the path without rendering a
    /// model-chosen query. The two are joined by [`Self::url_with_query`], which is what a transport calls.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the query parameters, in the order they will be sent.
    ///
    /// Ordered rather than a map, because a map's iteration order would make the same logical request produce
    /// different URLs on different runs — which makes a recorded fixture unusable and a signature over the
    /// request impossible.
    #[must_use]
    pub fn query(&self) -> &[(String, String)] {
        &self.query
    }

    /// Returns the `Accept` header value.
    #[must_use]
    pub const fn accept(&self) -> &'static str {
        self.accept
    }

    /// Renders the full URL, with the query appended.
    ///
    /// The values in `query` are already percent-encoded, so this joins rather than encodes again: encoding
    /// twice would turn `%20` into `%2520` and the provider would receive the literal text.
    #[must_use]
    pub fn url_with_query(&self) -> String {
        if self.query.is_empty() {
            return self.url.clone();
        }
        let pairs: Vec<String> = self
            .query
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        format!("{}?{}", self.url, pairs.join("&"))
    }
}

impl fmt::Display for HttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The query can hold model-chosen text, so the rendering reports the **shape**: the method, the path,
        // and the parameter NAMES. A parameter's name is this module's own constant; its value is not.
        let names: Vec<&str> = self.query.iter().map(|(name, _)| name.as_str()).collect();
        write!(
            formatter,
            "{} {} [{}]",
            self.method,
            self.url,
            names.join(", ")
        )
    }
}

#[cfg(test)]
impl HttpRequest {
    /// Retargets a request at a test origin, keeping the path and the encoded parameters.
    ///
    /// A **test seam**, and it is the only way a transport test can drive this crate's real requests: the
    /// builders are bound to the documented API bases, and a test must point at a loopback server instead. It
    /// rewrites the **origin alone**, so the path and the percent-encoded query a test asserts on are the ones
    /// the product builds — a seam that rebuilt the request would let the encoding drift unobserved.
    pub(crate) fn rebase_to(mut self, origin: &str) -> Self {
        const API_ORIGIN: &str = "https://www.googleapis.com";
        if let Some(rest) = self.url.strip_prefix(API_ORIGIN) {
            self.url = format!("{origin}{rest}");
        }
        self
    }
}

/// Percent-encodes a query value per RFC 3986.
///
/// The unreserved set is `ALPHA / DIGIT / "-" / "." / "_" / "~"`; everything else becomes `%XX` with
/// **uppercase** hex, which §6.2.2.1 normalisation requires (lowercase is equivalent but not canonical, so a
/// recorded fixture would differ from what the provider echoes).
///
/// # Why a space is `%20` and not `+`
///
/// `application/x-www-form-urlencoded` encodes a space as `+`, and Google accepts either. `%20` is chosen
/// because it is correct in **every** query position including a path segment, while `+` is only a space when
/// the receiver applies the form-urlencoded rule — so a value containing a literal `+` (which Gmail's search
/// syntax uses, for example in `larger:10M`) would be ambiguous to a receiver that decoded `+` as a space.
/// Encoding `+` as `%2B` and a space as `%20` removes the ambiguity in both directions.
#[must_use]
pub fn percent_encode(value: &str) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        if unreserved {
            encoded.push(char::from(*byte));
        } else {
            // ASCII hex, uppercase. `write!` into a `String` cannot fail, and it avoids the allocation an
            // appended `format!` would make for every escaped byte.
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

/// Validates a resource identifier.
///
/// # Errors
///
/// Returns [`RequestError::Argument`] when the value is empty, oversized, or holds a control character. The
/// control-character refusal matters beyond a malformed URL: an identifier reaches a log line, and a newline
/// in one forges a record.
fn resource_id<'a>(field: &'static str, value: &'a str) -> Result<&'a str, RequestError> {
    let trimmed: &str = value.trim();
    if trimmed.is_empty() {
        return Err(RequestError::Argument {
            field,
            reason: "an identifier may not be empty",
        });
    }
    if trimmed.chars().count() > MAX_RESOURCE_ID_CHARS {
        return Err(RequestError::Argument {
            field,
            reason: "an identifier may be at most 256 characters",
        });
    }
    if trimmed.chars().any(char::is_control) {
        return Err(RequestError::Argument {
            field,
            reason: "an identifier may not hold a control character, because it becomes a URL path \
                     segment and a log field",
        });
    }
    Ok(trimmed)
}

/// Validates a Gmail search query.
///
/// # Why it takes the field name
///
/// It was hard-coded to `"query"` and had **three** callers: the Gmail list's `q` parameter and Calendar's
/// `time_min` and `time_max`. So an oversized time bound produced *"the `query` argument is unusable"* for an
/// argument the caller never sent, and called an RFC 3339 instant "a search query". The field name is now an
/// argument, so the only thing that names the failing field is the caller that knows which one it is
/// (`ADR-0086`).
///
/// # Errors
///
/// Returns [`RequestError::Argument`] when the query is oversized or holds a control character. An empty
/// query is **accepted** and means "the newest messages", so it is not refused — the schema's own description
/// says so, and refusing it would make a legitimate request impossible.
fn search_query<'a>(field: &'static str, value: &'a str) -> Result<&'a str, RequestError> {
    if value.chars().count() > MAX_QUERY_CHARS {
        return Err(RequestError::Argument {
            field,
            reason: "a search query may be at most 512 characters",
        });
    }
    if value.chars().any(char::is_control) {
        return Err(RequestError::Argument {
            field,
            reason: "a search query may not hold a control character",
        });
    }
    Ok(value)
}

/// Validates an RFC 3339 time bound.
///
/// **A separate validator from [`search_query`] and not a reuse of it**, because the two are different kinds
/// of value and the refusal must say so. A time bound was previously run through `search_query`, which meant an
/// oversized `time_min` was refused with a message about *"a search query"* that named the argument
/// `query` — a field the caller had not supplied. The bound is the same figure by coincidence (both are
/// bounded by [`MAX_QUERY_CHARS`]), and the *reason* is what differs: a timestamp is a bounded string entering
/// a URL and a log line, not a provider's search syntax (`ADR-0086`).
///
/// # Errors
///
/// Returns [`RequestError::Argument`] when the bound is oversized or holds a control character. An empty bound
/// is accepted here and refused by the provider as `timeRangeEmpty`, so this does not duplicate that check —
/// the provider's own reason is the diagnostic.
fn time_bound<'a>(field: &'static str, value: &'a str) -> Result<&'a str, RequestError> {
    if value.chars().count() > MAX_TIME_BOUND_CHARS {
        return Err(RequestError::Argument {
            field,
            reason: "a time bound may be at most 64 characters",
        });
    }
    if value.chars().any(char::is_control) {
        return Err(RequestError::Argument {
            field,
            reason: "a time bound may not hold a control character, because it becomes a URL query value \
                     and a log field",
        });
    }
    Ok(value)
}

/// Validates `max_results` against an API's documented cap.
///
/// # Errors
///
/// Returns [`RequestError::MaxResults`] for zero or a value above the cap. **Zero is refused rather than read
/// as "unlimited"**, the reasoning every bound in this workspace records: a zero read as unlimited is an
/// unbounded request, and here it is also a request Google would answer with a default the caller did not
/// choose.
fn max_results(requested: u32, maximum: u32) -> Result<String, RequestError> {
    if requested == 0 || requested > maximum {
        return Err(RequestError::MaxResults { requested, maximum });
    }
    Ok(requested.to_string())
}

/// Adds a parameter only when it is present.
///
/// A parameter always added with an empty value would be sent as `name=`, which is a *different* request from
/// omitting it — and for `pageToken` the difference is between "the last page" and "a page token Google
/// rejects".
fn push(query: &mut Vec<(String, String)>, name: &str, value: Option<String>) {
    if let Some(value) = value {
        query.push((name.to_owned(), value));
    }
}

/// Builds the `users.messages.list` request.
///
/// # Errors
///
/// Returns [`RequestError`] for an unusable query, `max_results`, or page token.
pub fn gmail_messages_list(
    query: Option<&str>,
    max_results_value: Option<u32>,
    page_token: Option<&str>,
) -> Result<HttpRequest, RequestError> {
    let mut parameters = Vec::new();
    if let Some(text) = query {
        push(
            &mut parameters,
            "q",
            Some(percent_encode(search_query("query", text)?)),
        );
    }
    if let Some(value) = max_results_value {
        push(
            &mut parameters,
            "maxResults",
            Some(max_results(value, GMAIL_MAX_RESULTS_CAP)?),
        );
    }
    push(
        &mut parameters,
        "pageToken",
        client::next_page(page_token)?.map(|token| percent_encode(&token)),
    );
    Ok(HttpRequest {
        method: "GET",
        url: format!("{GMAIL_API_BASE}/users/me/messages"),
        query: parameters,
        accept: JSON_ACCEPT,
    })
}

/// The message formats the connector will ask for.
///
/// A closed enum rather than a string, and `Raw` is deliberately **absent**: `format=raw` returns the
/// unparsed MIME message including attachments, nothing in this connector parses it, and the tool's input
/// schema already omits it. A type that could not express `raw` is a stronger control than a check that
/// refuses it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageFormat {
    /// Identifiers, labels, and a snippet only.
    Minimal,
    /// Headers and labels.
    Metadata,
    /// The parsed body.
    Full,
}

impl MessageFormat {
    /// Returns the value Google expects for the `format` parameter.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Metadata => "metadata",
            Self::Full => "full",
        }
    }
}

/// Builds the `users.messages.get` request.
///
/// # Errors
///
/// Returns [`RequestError`] for an unusable message identifier.
pub fn gmail_messages_get(
    message_id: &str,
    format: MessageFormat,
) -> Result<HttpRequest, RequestError> {
    let id = resource_id("message_id", message_id)?;
    Ok(HttpRequest {
        method: "GET",
        // The identifier goes in the PATH, so it is percent-encoded as a path segment. The same encoding is
        // correct here as for a query value, because the unreserved set is identical — but it is applied for
        // a different reason: a `/` in an identifier would change which resource is addressed.
        url: format!("{GMAIL_API_BASE}/users/me/messages/{}", percent_encode(id)),
        query: vec![("format".to_owned(), format.as_str().to_owned())],
        accept: JSON_ACCEPT,
    })
}

/// Why a time range and a sync token cannot be combined, as the refusal's explanation.
///
/// A constant because the two checks below share one reason, and the wording names **both remedies**: a caller
/// has a real choice (drop the bounds to continue the sync, or drop the token for a filtered full read) rather
/// than only a mistake to undo.
const TIME_RANGE_WITH_SYNC_TOKEN: &str = "the provider refuses a time range together with a sync token, \
     because an incremental sync must repeat the initial request's filters; drop the time bounds to continue \
     the sync, or drop the sync token to do a filtered full read";

/// Builds the `events.list` request.
///
/// # A page token is required to continue a large incremental sync
///
/// The sync guide: "In cases where a large number of resources have changed since the last incremental sync
/// request, you may find a `pageToken` instead of a `syncToken` in the list result. In these cases you'll need
/// to perform the exact same list query as was used for retrieval of the first page in the incremental sync
/// (with the exact same `syncToken`), append the `pageToken` to it and paginate through all the following
/// requests until you find another `syncToken` on the last page."
///
/// So a page token and a sync token are sent **together** for the middle pages of an incremental walk, which is
/// the opposite of the `timeMin`/`timeMax` restriction below — the two parameters belong to different rules and
/// only one of them is a conflict. Without this parameter a large sync was **uncontinuable**: the renderer
/// emits `next_page_token`, the caller is told to fetch more, and no argument existed to do it with
/// (`ADR-0085`).
///
/// # A time range and a sync token cannot be sent together
///
/// The `events.list` reference lists the parameters that "cannot be specified together with nextSyncToken to
/// ensure consistency of the client state": `iCalUID`, `orderBy`, `privateExtendedProperty`, `q`,
/// `sharedExtendedProperty`, **`timeMin`**, **`timeMax`** and `updatedMin`. This builder offers two of those
/// eight — a time range — so it refuses the pairing rather than sending a request the provider documents as a
/// `400`. Of the other six, none is offered by [`calendar_events_list`]: it sends `calendarId`, `timeMin`,
/// `timeMax`, `maxResults`, `pageToken` and `syncToken` only, and `pageToken` is not on the restriction list.
///
/// **The refusal is local because the call could never succeed.** A `400` is a caller mistake that
/// `client::classify` already treats as permanent, so sending it would spend a request and a quota unit to
/// learn what the provider had already stated — and the caller would still not know which argument to drop. A
/// model reading the input schema sees both fields advertised, so the pairing is something it can plausibly
/// choose, and the honest answer is a refusal that names the pair (`ADR-0084`).
///
/// # Errors
///
/// Returns [`RequestError`] for an unusable calendar identifier, `max_results`, page token or sync token, and
/// [`RequestError::DisallowedCombination`] for a time range sent with a sync token.
pub fn calendar_events_list(
    calendar_id: &str,
    time_min: Option<&str>,
    time_max: Option<&str>,
    max_results_value: Option<u32>,
    page_token: Option<&str>,
    sync_token: Option<&str>,
) -> Result<HttpRequest, RequestError> {
    let calendar = resource_id("calendar_id", calendar_id)?;
    // Validated before the tokens so a caller learns about the unusable pair without a token having to be
    // parsed first — and because the pair is the more surprising fault of the two.
    let token = client::next_page(sync_token)?;
    // The page token is bounded for the same reason every token is: it becomes the next request's parameter,
    // and an oversized or control-bearing value would address a different page than the cursor records.
    let next_page_token = client::next_page(page_token)?;
    // The two checks are separate rather than one `matches!` over a tuple, so the refusal names **which** of the
    // two bounds conflicts. A single message saying "a time range" would leave a caller who sent only `timeMax`
    // wondering which argument to remove.
    if token.is_some() && time_min.is_some() {
        return Err(RequestError::DisallowedCombination {
            field: "time_min",
            other: "sync_token",
            reason: TIME_RANGE_WITH_SYNC_TOKEN,
        });
    }
    if token.is_some() && time_max.is_some() {
        return Err(RequestError::DisallowedCombination {
            field: "time_max",
            other: "sync_token",
            reason: TIME_RANGE_WITH_SYNC_TOKEN,
        });
    }
    let mut parameters = Vec::new();
    if let Some(value) = time_min {
        push(
            &mut parameters,
            "timeMin",
            Some(percent_encode(time_bound("time_min", value)?)),
        );
    }
    if let Some(value) = time_max {
        push(
            &mut parameters,
            "timeMax",
            Some(percent_encode(time_bound("time_max", value)?)),
        );
    }
    if let Some(value) = max_results_value {
        push(
            &mut parameters,
            "maxResults",
            Some(max_results(value, CALENDAR_MAX_RESULTS_CAP)?),
        );
    }
    // The page token is pushed **before** the sync token, matching the Gmail builders' order (`maxResults`,
    // `pageToken`) so the walk parameters appear together — and because the sync guide's own pagination example
    // is `…&syncToken=…&pageToken=…`, which is the shape a reader compares this against.
    push(
        &mut parameters,
        "pageToken",
        next_page_token.map(|token| percent_encode(&token)),
    );
    push(
        &mut parameters,
        "syncToken",
        token.map(|token| percent_encode(&token)),
    );
    Ok(HttpRequest {
        method: "GET",
        url: format!("{CALENDAR_API_BASE}/calendars/{calendar}/events"),
        query: parameters,
        accept: JSON_ACCEPT,
    })
}

/// Builds the `users.history.list` request.
///
/// The incremental-sync read: it returns the changes since `start_history_id` and, with them, the mailbox's
/// new `historyId`. Gmail charges 2 quota units here against `messages.list`'s 5, which is what makes an
/// incremental sync cheaper than a full one — and the reason a connector falls back to [`gmail_messages_list`]
/// when this answers a `404`.
///
/// `history_types` is deliberately **not offered**: the parameter would return only some change types, and a
/// sync that filtered would silently drop the changes it excluded. The general `messages` field is read
/// instead, so the walk sees every record the provider returned.
///
/// # Errors
///
/// Returns [`RequestError`] for an unusable `start_history_id`, `max_results`, or page token.
pub fn gmail_history_list(
    start_history_id: &str,
    max_results_value: Option<u32>,
    page_token: Option<&str>,
) -> Result<HttpRequest, RequestError> {
    // The starting position is validated as a resource identifier: it becomes a query parameter and a log
    // field, and an empty or control-bearing value would address a different position than the cursor records.
    let start = resource_id("start_history_id", start_history_id)?;
    let mut parameters = vec![("startHistoryId".to_owned(), percent_encode(start))];
    if let Some(value) = max_results_value {
        push(
            &mut parameters,
            "maxResults",
            Some(max_results(value, GMAIL_MAX_RESULTS_CAP)?),
        );
    }
    push(
        &mut parameters,
        "pageToken",
        client::next_page(page_token)?.map(|token| percent_encode(&token)),
    );
    Ok(HttpRequest {
        method: "GET",
        url: format!("{GMAIL_API_BASE}/users/me/history"),
        query: parameters,
        accept: JSON_ACCEPT,
    })
}

/// Builds the `users.getProfile` request: **who the credential belongs to.**
///
/// # Why this operation exists at all
///
/// `tools-and-connectors.md` requires an account's identity to be "verified from the provider, not user-entered
/// labels", and `P5-004`'s JARVIS Mapping names `users.getProfile`'s `emailAddress` as the value that satisfies
/// it. Nothing in the connector read it, so the requirement was **unimplementable** — an account could be
/// `VerifiedAccount`-shaped only if something supplied a provider account identifier, and no operation did.
///
/// # It takes no arguments, and that is the API's shape rather than a simplification
///
/// The reference gives the path as `/users/{userId}/profile` with `userId` documented as *"The user's email
/// address. The special value `me` can be used to indicate the authenticated user"*, and the **request body
/// must be empty**. The connector sends `me` — a constant, like every other builder on this path — because a
/// caller that could name another mailbox would be building a request its own token does not authorise. So
/// there is no `user_id` argument to validate and no combination to refuse: the only way to build this request
/// is to be the account it asks about.
///
/// # The cost, and why it is recorded on the operation rather than here
///
/// `getProfile` costs **1** quota unit, the cheapest call in this connector, and the reference's own quota
/// table is where that figure lives. The request builder states the URL; the operation declaration carries the
/// cost.
#[must_use]
pub fn gmail_profile() -> HttpRequest {
    HttpRequest {
        method: "GET",
        // `me`, spelled here rather than taken from an argument: see the doc above. No query parameters, so
        // `url_with_query` returns the path unchanged.
        url: format!("{GMAIL_API_BASE}/users/me/profile"),
        query: Vec::new(),
        accept: JSON_ACCEPT,
    }
}

/// A `POST` whose body is JSON, and authenticated by a bearer header.
///
/// # Why this is the shape for every body-bearing call here, and what it is not
///
/// [`HttpRequest`] has **no body** on purpose — every operation it builds is a `GET`, and it documents that a
/// body field "would be a shape nothing uses". Two operations disprove the general claim: `users.watch`
/// (`POST …/users/me/watch` with a JSON body carrying `topicName` and optionally `labelIds` and
/// `labelFilterBehavior`) and `channels.stop` (`POST …/calendar/v3/channels/stop` with a JSON body carrying
/// `id` and `resourceId`). So this crate needed a second request shape, and it is **not** [`FormRequest`] —
/// that type exists for the token endpoint, whose body carries the `code` and the `refresh_token`, is
/// `application/x-www-form-urlencoded`, and authenticates by its body's `client_id`. Neither body here carries
/// a credential, and both authenticate by the caller's **bearer header**, so reusing `FormRequest` would have
/// put a non-credential body into a type whose whole justification is the credential it holds.
///
/// # Why the name is `JsonRequest` rather than `WatchRequest`
///
/// It was `WatchRequest` while `users.watch` was its **only** caller. The second caller is not a watch —
/// `channels.stop` is the call that *ends* a channel that a watch created — so a name derived from its first
/// user became a claim about the type that is false. The axis that makes this a separate type from
/// `FormRequest` is the **credential boundary** (`ADR-0093`), not the operation, so the name follows the axis
/// rather than the caller. Renaming it is safe here precisely because it has no consumers: the builders are the
/// public surface, and the type is the value they return.
///
/// # What this type deliberately does not hold
///
/// **No header map, and therefore no credential in a field.** `HttpRequest` has no header field for the same
/// reason (`ADR-0060`): the bearer token is supplied by the transport binding at send time
/// ([`AUTHORIZATION_HEADER`]), so a request value cannot carry one. The only header a watch `POST` needs beyond
/// that is `Content-Type: application/json`, which is a constant returned by [`Self::content_type`] rather than
/// a field — a fact about the call rather than an argument to it.
///
/// # The body is already rendered
///
/// `body` holds JSON text produced by [`watch_json_body`] from values whose shape was checked first, so the
/// body is rendered **once** and by the function that also owns the field names. There is no `Serialize` here:
/// the body is a string, so the set of fields sent is visible in one place rather than spread across attributes
/// on a struct — which is what makes the deprecated-field rule below checkable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JsonRequest {
    url: String,
    body: String,
}

impl JsonRequest {
    /// Returns the absolute URL. It carries no query parameters: the watch takes its arguments in the body.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the rendered JSON body.
    ///
    /// **Not redacted, and that is a decision rather than an oversight.** A watch body holds a topic name and
    /// label ids: the topic name is a resource in the caller's own Cloud project, and the label ids are Gmail's
    /// own vocabulary (`INBOX`, `UNREAD`) rather than message content. A stop body holds a channel id the
    /// connector generated and a `resourceId` the provider returned — neither is a secret, and neither names a
    /// person. There is no credential here — unlike [`FormRequest::rendered_body`], whose type exists to hide
    /// one — so rendering it in a diagnostic shows what was sent without disclosing anything the caller did not
    /// already know. The body is bounded by the validators below, so it also cannot become an unbounded log
    /// line.
    #[must_use]
    pub fn rendered_body(&self) -> &str {
        &self.body
    }

    /// Returns the `Content-Type` the body is sent as.
    #[must_use]
    pub const fn content_type(&self) -> &'static str {
        JSON_ACCEPT
    }
}

/// Builds the `users.watch` request body.
///
/// # The three rules, and why each is enforced here rather than left to a caller
///
/// **1. A label filter only means something with a label list.** The reference: `labelIds` "dictates which
/// labels are required for a push notification to be generated", and `labelFilterBehavior` is "filtering
/// behavior of `labelIds` list specified". So `include`/`exclude` **govern a list, and with no list there is
/// nothing to govern** — the field is meaningless, and a request that sent it alone would be read by the
/// provider as an unfiltered watch while the caller believed it was filtered. That is a *silent* mistake: the
/// request succeeds, and the connector then receives every change when it asked for some. Refused with
/// [`RequestError::Ignored`], because that is exactly the failure mode.
///
/// **2. The deprecated field is refused outright.** The reference types `labelFilterAction` as
/// `enum (LabelFilterAction)` and says it is *"deprecated because it caused incorrect behavior in some
/// cases; use `labelFilterBehavior` instead"*, and of the newer field: *"This field replaces `labelFilterAction`;
/// if set, `labelFilterAction` is ignored."* So the two coexist in the schema, and sending the old one is not
/// an error at the provider — it is ignored when the new one is present and *incorrect* when it is not. The
/// builder therefore has **no parameter for it**: there is no way to ask this function to send it, which is
/// stronger than refusing a value, because a refusal can be widened by a later edit while an absent parameter
/// cannot be passed. The rule is documented here so a reader who noticed the schema field learns why it is
/// unreachable rather than assuming an omission.
///
/// **3. An empty label list is refused** rather than omitted, so "no filter" is expressed by passing `None` and
/// not by passing an empty `Vec`. The two would send the same request body (`labelIds` absent either way) after
/// the emptiness check below, so accepting the empty one would silently equate a value the caller built by
/// mistake with a deliberate choice — the "two situations, one rendering" defect this repository keeps
/// recording. An empty list is the shape a loop over zero labels produces, which is the case worth catching.
///
/// # Errors
///
/// Returns [`RequestError::Argument`] for an empty, oversized, or control-bearing topic name, or a list that is
/// empty or over [`MAX_WATCH_LABEL_IDS`], or a label id that is unusable; and [`RequestError::Ignored`] for a
/// filter behaviour supplied without a label list.
fn watch_json_body(
    topic_name: &str,
    label_ids: Option<&[String]>,
    filter: Option<client::LabelFilterBehavior>,
) -> Result<String, RequestError> {
    let topic = topic_name.trim();
    // A whitespace-only name is refused as well as an empty one: it satisfies "supplied" while denoting nothing,
    // and the same reading `account::VerifiedAccount::new` takes for a provider identifier.
    if topic.is_empty() || topic.chars().count() > MAX_TOPIC_NAME_CHARS {
        return Err(RequestError::Argument {
            field: "topic_name",
            reason: "a Cloud Pub/Sub topic name is 1 to 512 characters and not only whitespace, because it \
                     becomes a request body field and a log line",
        });
    }
    // **The control check is against the raw value, not the trimmed one, and this test found the difference.** A
    // validator that trims first and checks second removes a trailing `\n` before looking for one, so
    // `"projects/p/topics/t\n"` is silently accepted and sent as the clean string — the newline that the check
    // exists to catch is the one the trim deletes. Trimming is normalization and is right for the emptiness and
    // length checks; refusing any control character in what the caller actually supplied is the stricter
    // reading, and a topic name has no legitimate whitespace for it to reject.
    if topic_name.chars().any(char::is_control) {
        return Err(RequestError::Argument {
            field: "topic_name",
            reason: "a topic name may not hold a control character, because it becomes a request body field \
                     and a log line where a newline forges a record",
        });
    }
    // A label filter with no list to filter is the silent case: the provider generates notifications for every
    // change while the caller believes it scoped them.
    if filter.is_some() && label_ids.is_none() {
        return Err(RequestError::Ignored {
            field: "label_filter_behavior",
            reason: "it filters a `label_ids` list, so with no labels it governs nothing and the provider \
                     watches every change; send label ids, or omit the filter",
        });
    }
    let mut value = serde_json::json!({ "topicName": topic });
    match label_ids {
        None => {}
        Some(ids) => {
            if ids.is_empty() {
                return Err(RequestError::Argument {
                    field: "label_ids",
                    reason: "an empty label list means \"no filter\", which is what omitting the argument \
                             says; pass no labels rather than an empty list, so a filter the caller did not \
                             choose cannot render as one they did",
                });
            }
            if ids.len() > MAX_WATCH_LABEL_IDS {
                return Err(RequestError::Argument {
                    field: "label_ids",
                    reason: "a watch may be scoped to at most 64 labels",
                });
            }
            let mut validated = Vec::with_capacity(ids.len());
            for id in ids {
                let id = resource_id("label_ids", id)?;
                validated.push(serde_json::Value::String(id.to_owned()));
            }
            value["labelIds"] = serde_json::Value::Array(validated);
            // The behaviour is written **only** when it can govern a list, which the check above guarantees is
            // the case here — so the body can never carry a filter with nothing to filter.
            if let Some(behaviour) = filter {
                value["labelFilterBehavior"] =
                    serde_json::Value::String(behaviour.as_str().to_owned());
            }
        }
    }
    Ok(value.to_string())
}

/// Builds the `users.watch` request that registers a mailbox and starts a notification lease.
///
/// # What this is for
///
/// The push guide: *"To configure Gmail accounts to send notifications to your Cloud Pub/Sub topic, use your
/// Gmail API client to call the `watch` method on the Gmail user mailbox."* So this is the call that makes push
/// delivery exist at all, and its response is the one [`crate::google::watch::parse_watch_response`] reads: the
/// `historyId` anchor and the lease `expiration`.
///
/// # The three traps, and where each is handled
///
/// The **body** is a `POST` with JSON, which is why this returns [`JsonRequest`] rather than the GET-only
/// [`HttpRequest`]. The **deprecated `labelFilterAction`** has no parameter, so it is unreachable. And the
/// **filter-without-a-list** pairing is refused in [`watch_json_body`] rather than sent, because the provider
/// does not treat it as an error.
///
/// # Errors
///
/// Returns [`RequestError`] for an unusable topic name, label list, or filter pairing — see
/// [`watch_json_body`] for each rule.
pub fn gmail_watch(
    topic_name: &str,
    label_ids: Option<&[String]>,
    filter: Option<client::LabelFilterBehavior>,
) -> Result<JsonRequest, RequestError> {
    let body = watch_json_body(topic_name, label_ids, filter)?;
    Ok(JsonRequest {
        // Gmail's own base, and `me` because the connector authenticates as the account it watches. The
        // identifier is a constant here rather than an argument, because a caller that could name another
        // mailbox would be building a request the token does not authorise — a `403` rather than a watch.
        url: format!("{GMAIL_API_BASE}/users/me/watch"),
        body,
    })
}

/// Builds the `channels.stop` request that ends **one** Calendar notification channel.
///
/// # Why this is a per-channel call, and why there is no Gmail counterpart here
///
/// A Calendar channel is stopped by naming a **channel id and a resource id together** — the guide: *"This
/// method requires that you provide at least the channel's `id` and the `resourceId` properties"* — because the
/// channel id identifies the channel while the `resourceId` names **which watched resource** it is for, and the
/// guide says a channel *"is associated both with a particular user and a particular resource (or set of
/// resources)"*. The pair, not the id alone, is therefore a channel's identity, and this function takes both.
///
/// **There is no per-user form and so no single call**, which is the fact that makes a Calendar teardown
/// different in kind from a Gmail one: *"Note that if the Google Calendar API has several types of resources
/// that have `watch` methods, there's only one `stop` method"* — reached at `…/channels/stop`, one channel at a
/// time. An account with three watched calendars needs three calls, and missing one leaves that calendar
/// notifying until **its own** expiry (`crate::google::teardown`).
///
/// `users.stop` is deliberately **not** built beside this: it is a different method on a different collection
/// whose body is empty, so it would need a third request shape for no gain, and nothing calls it yet.
///
/// # The permission rule this cannot check
///
/// The guide: *"If the channel was created by a regular user account, only the same user from the same client
/// (as identified by the OAuth 2.0 client IDs from the auth tokens) who created the channel can stop the
/// channel. If the channel was created by a service account, any user from the same client can stop the
/// channel."* This builder cannot enforce that, because the client id is a claim **inside** the credential and
/// [`crate::google::credential`] deliberately exposes only a rendered header value — so the rule is recorded
/// as a limit rather than a check, and the provider's `403` is what a violation looks like.
///
/// # Errors
///
/// Returns [`RequestError::Argument`] for an unusable channel id or resource id, through the same
/// [`resource_id`] validator every other identifier here uses — so the two bodies are checked identically
/// rather than each in its own way.
pub fn calendar_channel_stop(
    channel_id: &str,
    watched_resource_id: &str,
) -> Result<JsonRequest, RequestError> {
    let id = resource_id("channel_id", channel_id)?;
    let watched = resource_id("resource_id", watched_resource_id)?;
    // Built as a map rather than through a struct so the two field names are spelled once each, in the same
    // place as the values they carry — which is what makes it checkable that `id` holds the channel id and
    // `resourceId` holds the resource id, since the two are opaque strings of similar shape and could be
    // transposed without either being malformed.
    let mut body = serde_json::Map::new();
    body.insert("id".to_owned(), serde_json::Value::String(id.to_owned()));
    body.insert(
        "resourceId".to_owned(),
        serde_json::Value::String(watched.to_owned()),
    );
    Ok(JsonRequest {
        url: format!("{CALENDAR_API_BASE}/channels/stop"),
        body: serde_json::Value::Object(body).to_string(),
    })
}

/// A `POST` whose body is a form, which is what the token endpoint requires.
///
/// # Why this is a separate type from [`HttpRequest`], and why it has no credential
///
/// [`HttpRequest`] exists around one property: it takes its query from the connector's own constants, so it
/// can never be given a *credential* to put in a URL (`ADR-0060`). A token request is the opposite case — its
/// body carries the `code` and the `refresh_token`, which are exactly the values that must not be printable —
/// so it **cannot** be the same type. A union would either give `HttpRequest` a field a credential goes in
/// (undoing `ADR-0060`) or force a caller to prove which kind it holds.
///
/// What this type does **not** have is a header map, for the same reason `HttpRequest` has none: the one header
/// a form `POST` needs is `content_type`, which is a field, and the token endpoint authenticates by the body's
/// `client_id` rather than by a bearer header. So there is no header a credential could reach either.
///
/// # The body is already rendered
///
/// `body` holds the bytes [`crate::form::encode_body`] produced, so the encoding happens once and at the layer
/// that renders it (`ADR-0072`). This type does not encode, and a caller cannot ask it to: it takes the
/// rendered text, which is what makes double-encoding unrepresentable rather than merely discouraged.
#[derive(Clone, Eq, PartialEq)]
pub struct FormRequest {
    url: String,
    body: String,
    content_type: &'static str,
}

impl FormRequest {
    /// Builds a form `POST` whose body has already been rendered.
    ///
    /// `content_type` is supplied by the caller from its own constant rather than defaulted here, because the
    /// media type belongs to the protocol the caller implements — a token request and, later, a revocation are
    /// both form `POST`s to the same host with the same type, and a default in this module would be a fact
    /// about one of them stated in a module that knows neither.
    #[must_use]
    pub fn new(
        url: impl Into<String>,
        body: impl Into<String>,
        content_type: &'static str,
    ) -> Self {
        Self {
            url: url.into(),
            body: body.into(),
            content_type,
        }
    }

    /// Returns the absolute URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Returns the rendered body.
    ///
    /// **This returns credential-bearing text.** It is named for what it is rather than `body` alone so a call
    /// site that renders it is visible, and the type has no `Display` for that reason.
    #[must_use]
    pub fn rendered_body(&self) -> &str {
        &self.body
    }

    /// Returns the `Content-Type` the body is sent as.
    #[must_use]
    pub const fn content_type(&self) -> &'static str {
        self.content_type
    }
}

impl fmt::Debug for FormRequest {
    /// Reports the shape and **not the body**, for the reason [`crate::google::credential::AccessToken`] has a
    /// hand-written `Debug`: a derived one would render whatever the struct holds, and this struct holds a
    /// credential-bearing form. The body's **length** is printed because it is not the value and it is what
    /// distinguishes two requests in a diagnostic.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "FormRequest {{ method: POST, url: {}, content_type: {}, body: [REDACTED], chars: {} }}",
            self.url,
            self.content_type,
            self.body.chars().count()
        )
    }
}

/// The bearer scheme a Google API request authenticates with.
///
/// A constant so the header's *name* and scheme appear once. The token itself is supplied by the transport
/// binding — so this module states the shape and holds no value. Present on the **read** path only: a form
/// `POST` authenticates by its body's `client_id` and has no bearer header (see [`FormRequest`]).
pub const AUTHORIZATION_HEADER: &str = "Authorization";

/// The scheme prefix the authorization header uses.
pub const BEARER_SCHEME: &str = "Bearer";

/// One page of identifiers, as a list response carries them.
///
/// Deliberately **not** the provider's resource: this carries what the connector's own output schema promises
/// (`message_ids` and `next_page_token`), so the two cannot drift. A response parser that returned Gmail's
/// `Message` resource would make the tool's declared output a fiction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdPage {
    /// The identifiers on this page.
    pub ids: Vec<String>,
    /// The token for the next page, when the provider sent one.
    pub next_page_token: Option<String>,
}

/// The `messages.list` response body.
#[derive(Clone, Debug, Deserialize)]
struct MessagesListBody {
    #[serde(default)]
    messages: Vec<MessageIdEntry>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

/// One entry of a `messages.list` response.
#[derive(Clone, Debug, Deserialize)]
struct MessageIdEntry {
    id: String,
}

/// The `users.getProfile` response, read for the fields this connector's output declares.
///
/// # Why `historyId` is read here as well as from `history.list`
///
/// The reference gives the profile as `{ "emailAddress": string, "messagesTotal": integer, "threadsTotal":
/// integer, "historyId": string }` — so a profile read yields **the mailbox's current position** without
/// consuming a message. That makes it an alternative way to establish a sync anchor, though *not* the same
/// one [`crate::google::watch::parse_watch_anchor`] supplies: both are the mailbox's position at the moment
/// they were read, and the sync guide's rule is that a full sync may store "the `historyId` of the most recent
/// message" — so **either** can seed a first sync, and neither is the position a sync *ends* at.
///
/// `emailAddress` is the field a caller needs and the one that must be present: it is what
/// `tools-and-connectors.md` requires "verified from the provider, not user-entered labels", and the response
/// is the provider's own statement of it. The three other fields are optional here because this connector
/// declares only the two it renders, and reading a field it does not promise would be parsing for a shape it
/// has no contract for.
#[derive(Clone, Debug, Deserialize)]
struct ProfileBody {
    #[serde(default, rename = "emailAddress")]
    email_address: Option<String>,
    #[serde(default, rename = "historyId")]
    history_id: Option<String>,
}

/// The parsed `users.getProfile` response, reduced to the fields this connector's output declares.
///
/// **The address is the identity and is the only required field**, so a body without one is refused rather
/// than returned with a `None`: the operation exists to establish *which* mailbox answered, and a profile with
/// no address establishes nothing. `history_id` stays optional because the reference documents it as a
/// separate field and a caller may want only the identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GmailProfile {
    /// The mailbox's address, as the provider stated it.
    pub email_address: String,
    /// The mailbox's position at the time of the read, when the provider returned one.
    pub history_id: Option<String>,
}

/// Parses a `users.getProfile` response.
///
/// # Errors
///
/// Returns [`RequestError`] when the status is not a success or the body is not JSON carrying an
/// `emailAddress`. The status is checked first, for the reason [`parse_id_page`] records: parsing a body
/// before checking the status is how an error document becomes an empty result.
///
/// **The address is not validated as an email address here.** Google's `emailAddress` is a string the provider
/// asserts about its own account; a local RFC 5322 parser would reject a valid-but-unusual address and turn a
/// successful read into a failure. What the connector does with the value is
/// [`crate::account::VerifiedAccount::new`]'s business, and that type applies the bound a *stored identifier*
/// needs.
pub fn parse_profile(status: u16, body: &str) -> Result<GmailProfile, RequestError> {
    if status != 200 {
        return Err(RequestError::Argument {
            field: "status",
            reason: "a profile response may only be parsed from a 200; every other status is an outcome for \
                     `client::classify` rather than an identity",
        });
    }
    let parsed: ProfileBody = serde_json::from_str(body).map_err(|_| RequestError::Argument {
        field: "body",
        reason: "a profile response must be a JSON object",
    })?;
    let email_address = parsed
        .email_address
        .filter(|address| !address.trim().is_empty())
        .ok_or(RequestError::Argument {
            field: "emailAddress",
            // A whitespace-only address is refused as well as an absent one: the operation exists to say which
            // mailbox answered, and a blank answers nothing while satisfying "the field was present".
            reason: "a profile must carry a non-empty `emailAddress`, because that value is the account's \
                     verified identity and a profile without one establishes nothing",
        })?;
    Ok(GmailProfile {
        email_address,
        history_id: parsed.history_id.filter(|id| !id.trim().is_empty()),
    })
}

/// The `messages.get` response body, read for the fields this connector's output declares.
///
/// A private struct rather than inline `Value` lookups, so the three field names and their `serde` renames
/// appear once — and so a field the resource renames is a compile-visible change to this struct rather than a
/// silently-`None` lookup.
///
/// `id` is `Option` so that a JSON body which is well-formed but **carries no `id`** is refused with a reason
/// about the missing field rather than one about the body not being JSON. Those are different defects and the
/// old parser kept them apart.
#[derive(Clone, Debug, Deserialize)]
struct MessageBody {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, rename = "threadId")]
    thread_id: Option<String>,
    // `Option<Vec<String>>` rather than `Vec<String>` so that **absent** stays distinct from **empty**: an
    // absent `labelIds` means the provider did not return the field, while `[]` means the message carries no
    // labels. A `Vec` with `#[serde(default)]` collapses both to empty, which would render "this message has no
    // labels" for a response that never mentioned labels at all.
    #[serde(default, rename = "labelIds")]
    label_ids: Option<Vec<String>>,
}

/// The `events.list` response body.
#[derive(Clone, Debug, Deserialize)]
struct EventsListBody {
    #[serde(default)]
    items: Vec<EventIdEntry>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
    #[serde(default, rename = "nextSyncToken")]
    next_sync_token: Option<String>,
}

/// One entry of an `events.list` response.
#[derive(Clone, Debug, Deserialize)]
struct EventIdEntry {
    id: String,
}

/// The `history.list` response body.
///
/// The `historyId` field is the mailbox's **new position**. Google's method reference says that when the
/// response carries no `nextPageToken` there are no further updates, "and you can store the returned
/// `historyId` for a future request" — so this field, not the page token, is what a sync cursor advances to.
#[derive(Clone, Debug, Deserialize)]
struct HistoryListBody {
    #[serde(default)]
    history: Vec<HistoryEntry>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
    #[serde(default, rename = "historyId")]
    history_id: Option<String>,
}

/// One entry of a `history.list` response.
///
/// Only `messages` is read. The reference documents `messagesAdded`/`messagesDeleted`/`labelsAdded`/
/// `labelsRemoved` as the specific change-type fields and recommends them over the general `messages` list —
/// but *recommends*, not requires, and `messages` is the field populated on every change. So a connector that
/// read only the change-type arrays would miss a record that carried just `messages`, and the two would
/// disagree about what changed.
#[derive(Clone, Debug, Deserialize)]
struct HistoryEntry {
    #[serde(default)]
    messages: Vec<MessageIdEntry>,
}

/// A parsed history page.
///
/// Carries the mailbox's stated position alongside the message ids the change set touched, as a
/// [`HistoryPosition`](client::HistoryPosition) rather than a bare id — because whether that id may be **stored**
/// depends on the **page token beside it**. The reference states the rule in `startHistoryId`'s description,
/// not in the response field's: *"If you receive no `nextPageToken` in the response, there are no updates to
/// retrieve and you can store the returned `historyId` for a future request."*
///
/// Three fields that were previously described as two kinds of token now have three distinct lifetimes, and
/// conflating any two of them loses either an unfinished walk or a mailbox that stated no position.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryPage {
    /// The message identifiers the change set touched.
    pub ids: Vec<String>,
    /// The token for the next page, when the change set was larger than one page.
    pub next_page_token: Option<String>,
    /// The mailbox's position, **qualified** by whether this page ended the walk.
    pub position: client::HistoryPosition,
}

/// A parsed Calendar page, which carries **two** continuation tokens and they are not interchangeable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalendarPage {
    /// The event identifiers on this page.
    pub ids: Vec<String>,
    /// The next **page**, valid only until the walk finishes.
    pub next_page_token: Option<String>,
    /// The next **sync token**, present only on the last page.
    ///
    /// A different thing from `next_page_token`: the page token continues *this* query, while the sync token
    /// positions a *future* incremental sync. Google's sync guide says the sync token "is present only on the
    /// very last page", so a caller that stored the page token as a cursor would store a token that expires
    /// with the walk.
    pub next_sync_token: Option<String>,
}

/// Parses a list response into identifiers.
///
/// `status` is checked first, because the alternative — parsing a body and then discovering the status — is
/// how an error document becomes an empty page. **An empty page and a failure must not both read as "no
/// results".**
///
/// # Errors
///
/// Returns [`RequestError`] when the status is not a success, when the body is not JSON of the expected shape,
/// or when a page token is unusable.
pub fn parse_id_page(status: u16, body: &str) -> Result<IdPage, RequestError> {
    if status != 200 {
        return Err(RequestError::Argument {
            field: "status",
            // The reason is the STATUS, not the body: a provider error message is external text, and this
            // module's own rule is that a classification comes from the status and the machine-readable
            // reason rather than from prose. `client::classify` is where the retry decision belongs.
            reason: "a list response may only be parsed from a 200; every other status is an outcome for \
                     `client::classify` rather than an empty page",
        });
    }
    let parsed: MessagesListBody =
        serde_json::from_str(body).map_err(|_| RequestError::Argument {
            field: "body",
            reason: "a list response body must be JSON with a `messages` array",
        })?;
    // The token is validated on the way out as well as on the way in, because a token from a *response* becomes
    // the next request's parameter. `client::next_page` is the one bound, applied in both directions.
    let next_page_token = client::next_page(parsed.next_page_token.as_deref())?;
    Ok(IdPage {
        ids: parsed.messages.into_iter().map(|entry| entry.id).collect(),
        next_page_token,
    })
}

/// Parses an `events.list` response.
///
/// # Errors
///
/// Returns [`RequestError`] as [`parse_id_page`] does.
pub fn parse_calendar_page(status: u16, body: &str) -> Result<CalendarPage, RequestError> {
    if status != 200 {
        return Err(RequestError::Argument {
            field: "status",
            reason: "an events response may only be parsed from a 200",
        });
    }
    let parsed: EventsListBody =
        serde_json::from_str(body).map_err(|_| RequestError::Argument {
            field: "body",
            reason: "an events response body must be JSON with an `items` array",
        })?;
    let next_page_token = client::next_page(parsed.next_page_token.as_deref())?;
    let next_sync_token = client::next_page(parsed.next_sync_token.as_deref())?;
    Ok(CalendarPage {
        ids: parsed.items.into_iter().map(|entry| entry.id).collect(),
        next_page_token,
        next_sync_token,
    })
}

/// Parses a `history.list` response.
///
/// `status` is checked first, for the reason every parser here records: an error document parsed as a page
/// reports "no changes", and a caller cannot then tell a mailbox with nothing new from a refused request.
///
/// **A `404` is deliberately not resolved here.** It is the cursor's staleness signal, and turning it into a
/// dead-cursor decision belongs to the caller through [`client::gmail_history_signal`] — a parser that decided
/// it would be inferring which method was called, which only the caller knows.
///
/// # Errors
///
/// Returns [`RequestError`] when the status is not a success, the body is not the documented shape, or a page
/// token is unusable.
pub fn parse_history_page(status: u16, body: &str) -> Result<HistoryPage, RequestError> {
    if status != 200 {
        return Err(RequestError::Argument {
            field: "status",
            reason: "a history response may only be parsed from a 200; every other status is an outcome \
                     for `client::classify` rather than an empty change set",
        });
    }
    let parsed: HistoryListBody =
        serde_json::from_str(body).map_err(|_| RequestError::Argument {
            field: "body",
            reason: "a history response body must be JSON with a `history` array",
        })?;
    // The page token is bounded here because it becomes the next request's parameter. The `historyId` is
    // **not** bounded here: it becomes a cursor through `SyncCursor::new`, which applies its own bound, and a
    // second check would be a bound enforced in two places.
    let next_page_token = client::next_page(parsed.next_page_token.as_deref())?;
    Ok(HistoryPage {
        ids: parsed
            .history
            .into_iter()
            .flat_map(|entry| entry.messages)
            .map(|message| message.id)
            .collect(),
        position: client::HistoryPosition::of_page(parsed.history_id, next_page_token.as_deref()),
        next_page_token,
    })
}

/// The message fields this connector's `gmail_messages_read` output promises.
///
/// **This is not Gmail's `Message` resource.** It carries exactly the top-level fields the tool's declared
/// output names — the same rule [`IdPage`] and [`CalendarPage`] follow — so the declaration and the rendering
/// cannot drift. A parser returning the provider's `Message` would make the tool's declared output a fiction
/// (`ADR-0059`).
///
/// # Why there is no `snippet`, and why that is the finding rather than an omission
///
/// The `Message` resource has a `snippet` field, and an earlier version of the declared output promised it.
/// But the **Format** page defines what each format returns: `minimal` is "only email message ID and labels"
/// and `metadata` is "only email message ID, labels, and email headers" — neither returns `snippet`. So a tool
/// offering those two formats **cannot deliver** a `snippet` field, and declaring one promises a value two of
/// its three accepted formats never produce. The field is omitted rather than declared-optional, because
/// "sometimes absent" for something two of three formats never return reads as an unreliable field rather than
/// as an impossible one (`ADR-0083`).
///
/// # Why `thread_id` and `label_ids` are optional and `id` is not
///
/// `id` is returned by every format and is the one field every read can deliver. `thread_id` and `label_ids`
/// are in the resource, but the connector does not claim the provider returns them for **every** format — the
/// Format page names `labelIds` for `minimal` and `metadata` and says nothing about `threadId` — so they are
/// modelled as possibly-absent and the renderer emits each only when it arrived. A `required` field a
/// `minimal` read could not fill would make an honest response fail the tool's own validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GmailMessage {
    /// The immutable message identifier, present in every format.
    pub id: String,
    /// The thread the message belongs to, when the provider returned it.
    pub thread_id: Option<String>,
    /// The labels applied to the message, when the provider returned them.
    ///
    /// `None` means **the field was not in the response**, which is different from `Some(vec![])` meaning the
    /// message carries no labels. See [`MessageBody`] for why the two are kept apart.
    pub label_ids: Option<Vec<String>>,
}

/// Parses a `messages.get` response into the fields this connector's output declares.
///
/// # Errors
///
/// Returns [`RequestError`] when the status is not 200 or the body is JSON with no usable `id`.
pub fn parse_single_message(status: u16, body: &str) -> Result<GmailMessage, RequestError> {
    if status != 200 {
        return Err(RequestError::Argument {
            field: "status",
            reason: "a resource may only be read from a 200",
        });
    }
    let parsed: MessageBody = serde_json::from_str(body).map_err(|_| RequestError::Argument {
        field: "body",
        reason: "a resource body must be JSON",
    })?;
    let id = parsed.id.ok_or(RequestError::Argument {
        field: "body",
        reason: "a resource body must carry a string `id`",
    })?;
    Ok(GmailMessage {
        id,
        thread_id: parsed.thread_id,
        label_ids: parsed.label_ids,
    })
}

/// The API base a request belongs to, for a diagnostic that must not print a query.
#[must_use]
pub fn api_host_of(url: &str) -> Option<&'static str> {
    if url.starts_with(GMAIL_API_BASE) {
        Some("gmail")
    } else if url.starts_with(CALENDAR_API_BASE) {
        Some("calendar")
    } else {
        None
    }
}

/// Returns the connector's own identifier, so a diagnostic can name the connector that issued a request.
#[must_use]
pub const fn connector_id() -> &'static str {
    CONNECTOR_ID
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
