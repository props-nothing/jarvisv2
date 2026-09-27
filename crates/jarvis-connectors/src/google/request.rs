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
use serde_json::Value;

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
    /// A page token is unusable.
    #[error(transparent)]
    PageToken(#[from] client::PageTokenError),
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
/// # Errors
///
/// Returns [`RequestError::Argument`] when the query is oversized or holds a control character. An empty
/// query is **accepted** and means "the newest messages", so it is not refused — the schema's own description
/// says so, and refusing it would make a legitimate request impossible.
fn search_query(value: &str) -> Result<&str, RequestError> {
    if value.chars().count() > MAX_QUERY_CHARS {
        return Err(RequestError::Argument {
            field: "query",
            reason: "a search query may be at most 512 characters",
        });
    }
    if value.chars().any(char::is_control) {
        return Err(RequestError::Argument {
            field: "query",
            reason: "a search query may not hold a control character",
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
            Some(percent_encode(search_query(text)?)),
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

/// Builds the `events.list` request.
///
/// # Errors
///
/// Returns [`RequestError`] for an unusable calendar identifier, `max_results`, or sync token.
pub fn calendar_events_list(
    calendar_id: &str,
    time_min: Option<&str>,
    time_max: Option<&str>,
    max_results_value: Option<u32>,
    sync_token: Option<&str>,
) -> Result<HttpRequest, RequestError> {
    let calendar = resource_id("calendar_id", calendar_id)?;
    let mut parameters = Vec::new();
    if let Some(value) = time_min {
        push(
            &mut parameters,
            "timeMin",
            Some(percent_encode(search_query(value)?)),
        );
    }
    if let Some(value) = time_max {
        push(
            &mut parameters,
            "timeMax",
            Some(percent_encode(search_query(value)?)),
        );
    }
    if let Some(value) = max_results_value {
        push(
            &mut parameters,
            "maxResults",
            Some(max_results(value, CALENDAR_MAX_RESULTS_CAP)?),
        );
    }
    push(
        &mut parameters,
        "syncToken",
        client::next_page(sync_token)?.map(|token| percent_encode(&token)),
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
/// Carries the **new `historyId`** alongside the message ids the change set touched. The page token and the
/// history id are **different fields with different lifetimes** and are kept apart for the same reason
/// `CalendarPage` keeps its two tokens apart: the page token continues *this* walk (and expires when it ends),
/// while the history id is the durable position a *future* sync starts from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryPage {
    /// The message identifiers the change set touched.
    pub ids: Vec<String>,
    /// The token for the next page, when the change set was larger than one page.
    pub next_page_token: Option<String>,
    /// The mailbox's **new** position, which is the next sync cursor.
    pub history_id: Option<String>,
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
        next_page_token,
        history_id: parsed.history_id,
    })
}

/// Returns the JSON body's `id` field, for an operation whose output names one identifier.
///
/// # Errors
///
/// Returns [`RequestError`] when the status is not 200 or the body has no usable `id`.
pub fn parse_single_id(status: u16, body: &str) -> Result<String, RequestError> {
    if status != 200 {
        return Err(RequestError::Argument {
            field: "status",
            reason: "a resource may only be read from a 200",
        });
    }
    let value: Value = serde_json::from_str(body).map_err(|_| RequestError::Argument {
        field: "body",
        reason: "a resource body must be JSON",
    })?;
    value
        .get("id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or(RequestError::Argument {
            field: "body",
            reason: "a resource body must carry a string `id`",
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
