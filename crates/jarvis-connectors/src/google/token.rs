//! The Google token endpoint: what the exchange and refresh requests carry, and what an answer means.
//!
//! # Why the request is a parameter list rather than an [`HttpRequest`]
//!
//! [`HttpRequest`](crate::google::request::HttpRequest) shapes a **read**: a `GET`, query parameters, no body,
//! and no field for a credential (`ADR-0060`). A token request is none of those things — RFC 6749 §4.1.3 and
//! §6 make it a `POST` with an `application/x-www-form-urlencoded` body, and the credential is *inside* that
//! body rather than in a header. So this module builds a **parameter list**, not an `HttpRequest`, and the
//! distinction is deliberate: forcing a credential-bearing form into a type whose whole point is to be
//! renderable without a credential would undo the boundary `ADR-0060` and `ADR-0061` built.
//!
//! What that costs is stated rather than hidden: the parameter list has no `Display` and is not `Serialize`,
//! and the caller puts it on the wire. What it keeps is the part worth testing without a socket — **which
//! parameters, with what encoding, and what an answer means**.
//!
//! # The two rules this module exists to get right
//!
//! - **RFC 6749 §5.1 makes the response BODY authoritative, not the status.** A failure carries `error` and
//!   omits the token parameters, so a `400` **with** an `error` is the documented refusal while a `400`
//!   *without* one is a proxy's page or a misrouted request. Reading the status first would turn the second
//!   case into "the server said no", attributing a decision to a server that never made one. This is the
//!   mirror of the API path, where a `403`'s meaning comes from its status and reason code; here the protocol
//!   makes the body the authority, and the status still says whether the *provider* is unwell.
//! - **The access token must be as unreachable as the refresh token.** `ADR-0061` built `AccessToken` so the
//!   material cannot be rendered or stored, and this module must not undo that by producing an owned copy of
//!   it. So **this module never reads `access_token` into a value at all**: it reports its *presence*, and
//!   the caller reaches the bytes through the closure-based [`Secret`] on the token endpoint's own answer
//!   shape if it needs them. An earlier draft of this file read it into a `String` "and dropped it", which
//!   satisfies the letter of the rule and breaks its purpose — a copy existed, with a lifetime.
//! # What is not built
//!
//! Nothing performs the request: `reqwest` is used by the **read** path's transport and a token request is a
//! form `POST`, which that port does not express (`HttpMethod` has one variant). So the caller drives the
//! exchange — but every piece it needs is here and tested: the endpoint, the parameter lists, the body
//! encoding, and the answer reader. [`body`] is the seam a transport will call.

use serde_json::Value;

// `RefreshOutcome` is used by this module's tests and by the doc prose above; the production path reaches it
// only through `RefreshExchange::classify`, which is why the import is `cfg(test)`-free but the compiler
// needed a real use — the tests supply it.
#[cfg(test)]
use crate::auth::RefreshOutcome;
use crate::auth::{AuthError, ScopeSetChange};
use crate::token::{
    MAX_ACCESS_TOKEN_SECONDS, TokenEndpointFailure, TokenRequestOutcome, TokenResponse, TokenSet,
    split_scope,
};

/// The `grant_type` for an authorization-code exchange.
pub const GRANT_TYPE_AUTHORIZATION_CODE: &str = "authorization_code";

/// The `grant_type` for a refresh.
pub const GRANT_TYPE_REFRESH_TOKEN: &str = "refresh_token";

/// The shortest a PKCE `code_verifier` may be.
///
/// RFC 7636 §4.1: "A minimum length of 43 characters and a maximum length of 128 characters."
pub const MIN_CODE_VERIFIER_CHARS: usize = 43;

/// The longest a PKCE `code_verifier` may be.
pub const MAX_CODE_VERIFIER_CHARS: usize = 128;

/// The longest a refresh token may be, in characters.
///
/// A JARVIS bound rather than a provider figure: Google publishes none. It exists so an operator- or
/// provider-supplied string cannot become an unbounded request body, and it is generous enough that no real
/// token is refused.
pub const MAX_REFRESH_TOKEN_CHARS: usize = 4096;

/// The longest an authorization code may be, in characters.
///
/// The same kind of bound, and its own reason: a code arrives from a **redirect** rather than from a caller, so
/// an unbounded one would be attacker-supplied text with a length nobody chose.
pub const MAX_AUTHORIZATION_CODE_CHARS: usize = 4096;

/// The `grant_type` parameter name.
pub const GRANT_TYPE_PARAMETER: &str = "grant_type";

/// The `code` parameter name.
pub const CODE_PARAMETER: &str = "code";

/// The `code_verifier` parameter name.
pub const CODE_VERIFIER_PARAMETER: &str = "code_verifier";

/// The `refresh_token` parameter name.
pub const REFRESH_TOKEN_PARAMETER: &str = "refresh_token";

/// The `client_id` parameter name.
pub const CLIENT_ID_PARAMETER: &str = "client_id";

/// The `redirect_uri` parameter name.
pub const REDIRECT_URI_PARAMETER: &str = "redirect_uri";

/// Why a token request could not be constructed, or an answer could not be read.
///
/// A **refusal is not a variant of this type.** RFC 6749 §5.2 makes a refusal a well-formed response, so it is
/// [`TokenEndpointAnswer::Refused`] — the same split `jarvis-tools`' `AdapterError` draws between "I could not
/// run" and "here is what happened when I did".
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TokenRequestError {
    /// A parameter is unusable.
    #[error("the `{field}` parameter is unusable: {reason}")]
    Argument {
        /// Which parameter.
        field: &'static str,
        /// What is wrong.
        reason: &'static str,
    },
    /// The answer's body could not be read as a token response.
    #[error("the token endpoint's answer could not be read: {reason}")]
    Body {
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The answer was readable and describes a grant this platform will not accept.
    ///
    /// Distinct from [`Self::Body`] because the response did arrive and *was* understood: a `token_type` that is
    /// not `Bearer`, or a lifetime beyond the accepted bound, is a statement by the server rather than a
    /// decoding failure. One variant for both would send a reader to the wrong place.
    #[error("the token endpoint granted something this platform cannot use: {reason}")]
    UnusableGrant {
        /// What is wrong with it.
        reason: &'static str,
    },
}

/// A credential-shaped string that cannot be rendered or serialized.
///
/// Hand-written `Debug` and **no `Serialize`**, for the reason `ADR-0061` records: a credential reaches an
/// artifact through an *impl*, not through a data structure. The character count is reported because it is not
/// the value and it is what distinguishes two credentials in a diagnostic.
///
/// **Its `Debug` is asserted, not asserted-to-be-nice**: a test renders one and checks the value is absent,
/// because a hand-written `Debug` is exactly the kind of thing a later derive would silently replace.
#[derive(Clone, Eq, PartialEq)]
pub struct Secret {
    value: String,
}

/// A credential's declared shape, so the refusals are one function rather than four copies.
///
/// The refusals are ordered by **actionability**, not by position: an empty value and a whitespace-containing
/// value have different remedies ("supply one" vs "a paste carried a newline"), so they are separate messages
/// even though a single "invalid" would be shorter.
///
/// # Errors
///
/// Returns [`TokenRequestError::Argument`] for an empty, oversized, whitespace-containing, or
/// control-containing value. A credential is a bearer string, so **internal** whitespace is never legitimate —
/// and a trailing newline from a paste is the specific case this catches, because a credential that arrives
/// that way reaches the provider as a different string and surfaces as a generic auth failure.
pub fn validate_credential(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), TokenRequestError> {
    if value.trim().is_empty() {
        return Err(TokenRequestError::Argument {
            field,
            reason: "it is empty",
        });
    }
    if value.chars().count() > maximum {
        return Err(TokenRequestError::Argument {
            field,
            reason: "it is longer than this connector will send",
        });
    }
    if value.chars().any(char::is_whitespace) {
        return Err(TokenRequestError::Argument {
            field,
            reason: "it contains whitespace, which a bearer credential never does; a value pasted from a file \
                     or a terminal frequently carries a trailing newline",
        });
    }
    if value.chars().any(char::is_control) {
        return Err(TokenRequestError::Argument {
            field,
            reason: "it contains a control character, which a bearer credential never does",
        });
    }
    Ok(())
}

impl Secret {
    /// Wraps a credential.
    ///
    /// # Errors
    ///
    /// As [`validate_credential`].
    pub fn new(
        field: &'static str,
        value: impl Into<String>,
        maximum: usize,
    ) -> Result<Self, TokenRequestError> {
        let value = value.into();
        validate_credential(field, &value, maximum)?;
        Ok(Self { value })
    }

    /// Returns the character count, which is not the value.
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.value.chars().count()
    }

    /// Reaches the material through a closure.
    ///
    /// A closure rather than a `&str` return, so the borrow cannot outlive the call and the material cannot be
    /// moved somewhere a later `Debug` reaches it. The closure receives only the text, not the `Secret`, so
    /// reaching the bytes requires writing the word `exposed` — the greppable exposure site `ADR-0061` uses.
    pub fn with_exposed<T>(&self, use_it: impl FnOnce(&str) -> T) -> T {
        use_it(&self.value)
    }
}

impl std::fmt::Debug for Secret {
    /// Prints the shape and never the value.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Secret")
            .field("value", &"[REDACTED]")
            .field("chars", &self.char_len())
            .finish()
    }
}

/// What the token endpoint answered.
///
/// **One of two things**, and they need different responses from a caller: a grant to use, or a refusal to
/// classify. A `Result` with one error type could express a decoding failure but not the difference between
/// these, which is why a refusal is a variant rather than an error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TokenEndpointAnswer {
    /// The endpoint granted a token.
    Granted(Granted),
    /// The endpoint refused, with a protocol error code.
    Refused(TokenEndpointFailure),
}

/// A granted token response.
///
/// # Why there is no access token field
///
/// Because there must not be one. `ADR-0061` made the material reachable only through a closure so that
/// reaching it is a visible act, and `TokenSet` deliberately has no field for it. A field here holding the text
/// would be a **second, unredacted copy** of the same credential in a value that the rest of this crate
/// `Debug`-prints — so this type records that a token *arrived* (and how long it lives) and leaves the bytes
/// with the transport, which is the only component that needs them. `parse_answer` is given the whole body and
/// therefore *has* the text in scope; it reports the shape and lets the body fall out of scope, and a test
/// asserts that no field of this type can carry it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Granted {
    /// The response's non-secret fields.
    pub response: TokenResponse,
    /// The refresh token, when one arrived.
    ///
    /// Google's native-app documentation says `refresh_token` is "always returned for installed applications",
    /// so an exchange that omits it is unusual — but it is **optional rather than required**, because the
    /// provider conditions issuance on a risk assessment (RFC 6749 §4.14.2) and requiring it would fail a flow
    /// that actually succeeded.
    pub refresh_token: Option<Secret>,
}

impl TokenEndpointAnswer {
    /// Returns the response's non-secret fields, when the endpoint granted one.
    ///
    /// The accessor [`crate::token::RefreshExchange::classify`] takes, so a refresh's answer and its
    /// classification meet in one place rather than being re-derived at a call site.
    #[must_use]
    pub fn response(&self) -> Option<&TokenResponse> {
        match self {
            Self::Granted(granted) => Some(&granted.response),
            Self::Refused(_) => None,
        }
    }

    /// Returns the refusal, when the endpoint refused.
    ///
    /// **Exactly one of [`Self::response`] and this is `Some`**, which is what makes "the provider said no" and
    /// "the provider answered" impossible to confuse at a call site. A test asserts that disjointness for both
    /// variants, because it is the property the whole split rests on.
    #[must_use]
    pub fn failure(&self) -> Option<&TokenEndpointFailure> {
        match self {
            Self::Granted(_) => None,
            Self::Refused(failure) => Some(failure),
        }
    }

    /// Returns the refresh material, when the endpoint sent any.
    #[must_use]
    pub fn refresh_token(&self) -> Option<&Secret> {
        match self {
            Self::Granted(granted) => granted.refresh_token.as_ref(),
            Self::Refused(_) => None,
        }
    }

    /// Builds the usable token set, or `None` for a refusal.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::Flow`] when the grant is one this platform cannot use — the refusals
    /// [`TokenSet::from_response`] makes, surfaced here so a caller does not have to know the check lives in
    /// another module.
    pub fn token_set(
        &self,
        refresh_reference: Option<jarvis_core::SecretRef>,
        previous_scopes: &[String],
        requested_scopes: &[String],
    ) -> Result<Option<TokenSet>, AuthError> {
        match self.response() {
            None => Ok(None),
            Some(response) => Ok(Some(TokenSet::from_response(
                response,
                refresh_reference,
                previous_scopes,
                requested_scopes,
            )?)),
        }
    }

    /// Classifies this answer as the result of a **refresh**.
    ///
    /// Exists so the rotation rule is expressed once. The classification itself is
    /// [`crate::token::RefreshExchange::classify`]'s — this only supplies the two inputs a transport cannot
    /// supply from a socket: the stored reference, and the connector's vendor knowledge about revocation.
    ///
    /// # The doc used to claim a delegation the code did not make
    ///
    /// That sentence was **false** until this function was reduced to it: the body called a private
    /// `refresh_outcome` helper which restated the rotation test and the ordering — including the same comment
    /// about the transient check coming first. Two implementations of one rule, with a doc naming the other as
    /// the authority. It surfaced when a mutation to the **shared** classifier survived every test in this
    /// crate, because no path here called it. Removing the copy also removes the two inputs it restated: the
    /// accessors below are exactly the pair `classify` takes, and it computes `rotated_reference` itself, so a
    /// caller cannot disagree with it about whether a rotation happened.
    ///
    /// This is `P5-004`'s defect class — **a doc comment saying "the rule lives in X" is a claim about code**,
    /// and X was the wrong place because the code never went there.
    #[must_use]
    pub fn classify_refresh(
        &self,
        new_reference: Option<jarvis_core::SecretRef>,
        previous_scopes: &[String],
        requested_scopes: &[String],
        vendor_says_revoked: bool,
    ) -> crate::token::RefreshExchange {
        crate::token::RefreshExchange::classify(
            // The disjoint pair `RefreshExchange::classify` takes, which is why `response`/`failure` exist as
            // separate accessors: exactly one is `Some`, so "the provider granted" and "the provider refused"
            // cannot be confused at the call site.
            self.response(),
            self.failure(),
            new_reference,
            previous_scopes,
            requested_scopes,
            vendor_says_revoked,
        )
    }
}

/// The client identity and redirect a token request carries.
///
/// # Why the client identifier is a parameter and the secret is not
///
/// `P5-005`'s manifest declares **no secret fields**, because a PKCE public client identifies itself by an
/// identifier that is not secret. So there is no `client_secret` parameter here at all — not an optional one a
/// caller is trusted to leave unset, but **no field to put it in**. Google documents `client_secret` as
/// optional on both exchanges, and a connector that could send one would be a connector whose `secret_fields`
/// list and whose request disagree.
///
/// The redirect is part of the identity because Google requires `redirect_uri` to "exactly match one of the
/// authorized redirect URIs", so it is a *request* parameter rather than a client setting — and a mismatched
/// one is the `redirect_uri_mismatch` error rather than a transport failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExchangeIdentity {
    /// The public client identifier.
    pub client_id: String,
    /// The redirect URI, which must match a registered one exactly.
    pub redirect_uri: String,
}

impl ExchangeIdentity {
    /// Builds the identity from a client identifier and a redirect.
    ///
    /// # Errors
    ///
    /// Returns [`TokenRequestError::Argument`] for an empty client identifier, because an empty one produces a
    /// `400` whose code is about the client rather than about anything a reader could see.
    pub fn new(
        client_id: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Result<Self, TokenRequestError> {
        let client_id = client_id.into();
        if client_id.trim().is_empty() {
            return Err(TokenRequestError::Argument {
                field: "client_id",
                reason: "it is empty; a public client still has to name itself",
            });
        }
        Ok(Self {
            client_id,
            redirect_uri: redirect_uri.into(),
        })
    }

    /// Returns the parameters every token request carries, as **raw values**.
    ///
    /// # Why these are not pre-encoded
    ///
    /// They were, via `google::request::percent_encode`, and that was a live trap rather than a redundancy:
    /// the form body encoder ([`crate::form::encode_body`]) escapes every parameter it is given, so a value
    /// that arrived already escaped would be escaped **again** — `http://127.0.0.1/` becoming
    /// `http%253A%252F%252F127.0.0.1%252F` — and Google answers a double-encoded `redirect_uri` with
    /// `redirect_uri_mismatch`, an error a reader would chase through client registration rather than
    /// through the encoding.
    ///
    /// **The encoding belongs to exactly one layer, and that layer is the one that renders the body.** A list
    /// of values with an encoding already applied is a half-rendered request: every consumer must know whether
    /// it has been rendered, and the one that guesses wrong produces this failure. So this returns text, and
    /// the text is escaped once, by whoever produces the bytes.
    ///
    /// A test asserts the whole pipeline for the reason above: the values are raw here, and
    /// `encode_body(&identity.public_parameters())` contains exactly one level of escaping.
    fn parameters(&self) -> Vec<(String, String)> {
        vec![
            (CLIENT_ID_PARAMETER.to_owned(), self.client_id.clone()),
            (REDIRECT_URI_PARAMETER.to_owned(), self.redirect_uri.clone()),
        ]
    }

    /// Returns the identity parameters, for a caller assembling a request of its own.
    ///
    /// **Raw values**, and the caller that renders a body must encode them — see [`Self::parameters`].
    #[must_use]
    pub fn public_parameters(&self) -> Vec<(String, String)> {
        self.parameters()
    }

    /// Returns the endpoint this identity exchanges against.
    ///
    /// Taken from the manifest rather than restated, so the exchange cannot be pointed at one host while the
    /// manifest declares another. `P5-004` recorded the unusual detail this preserves: Google serves the
    /// consent screen and the token exchange from **different hosts**, so a reader "tidying" them into one
    /// constant would break every exchange.
    #[must_use]
    pub fn endpoint(&self) -> &'static str {
        crate::google::GoogleConnector::token_endpoint()
    }
}

/// Builds the parameter list for an authorization-code exchange.
///
/// The parameters Google documents are `code`, `client_id`, `redirect_uri`, `grant_type`, and `code_verifier`.
///
/// # Errors
///
/// Returns [`TokenRequestError::Argument`] when the code is unusable or the verifier is outside RFC 7636's
/// 43–128 character bounds or its unreserved alphabet.
pub fn exchange_code(
    code: &Secret,
    code_verifier: &Secret,
    identity: &ExchangeIdentity,
) -> Result<Vec<(String, String)>, TokenRequestError> {
    let verifier = code_verifier.with_exposed(|value| {
        if value.chars().count() < MIN_CODE_VERIFIER_CHARS
            || value.chars().count() > MAX_CODE_VERIFIER_CHARS
        {
            return Err(TokenRequestError::Argument {
                field: "code_verifier",
                reason: "RFC 7636 §4.1 requires a PKCE verifier of 43 to 128 characters",
            });
        }
        // The verifier is a credential-shaped value: it is the pre-image of the challenge the provider hashes
        // and compares. So it is checked against the RFC's own unreserved set rather than accepted as arbitrary
        // text, because a verifier that cannot be re-encoded identically produces a challenge mismatch that
        // looks like a PKCE bug rather than a malformed input.
        if !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
        {
            return Err(TokenRequestError::Argument {
                field: "code_verifier",
                reason: "RFC 7636 §4.1 defines it over `ALPHA / DIGIT / \"-\" / \".\" / \"_\" / \"~\"` only",
            });
        }
        Ok(value.to_owned())
    })?;

    let mut parameters = identity.parameters();
    parameters.push((
        GRANT_TYPE_PARAMETER.to_owned(),
        GRANT_TYPE_AUTHORIZATION_CODE.to_owned(),
    ));
    parameters.push((CODE_PARAMETER.to_owned(), code.with_exposed(str::to_owned)));
    parameters.push((CODE_VERIFIER_PARAMETER.to_owned(), verifier));
    Ok(parameters)
}

/// Builds the parameter list for a refresh.
///
/// Google's documented refresh carries `client_id`, `client_secret` (optional), `grant_type`, and
/// `refresh_token`. The secret is absent because this connector has no field for one.
#[must_use]
pub fn refresh(refresh_token: &Secret, identity: &ExchangeIdentity) -> Vec<(String, String)> {
    let mut parameters = identity.parameters();
    parameters.push((
        GRANT_TYPE_PARAMETER.to_owned(),
        GRANT_TYPE_REFRESH_TOKEN.to_owned(),
    ));
    parameters.push((
        REFRESH_TOKEN_PARAMETER.to_owned(),
        refresh_token.with_exposed(str::to_owned),
    ));
    parameters
}

/// Renders a parameter list as the body RFC 6749 §4.1.3 requires.
///
/// The single place the exchange's values become bytes, and therefore the single place they are escaped —
/// which is why the parameter lists hold **raw** values ([`ExchangeIdentity::parameters`]). A caller pairs this
/// with [`content_type`] when it builds the request; nothing here sends it, because a form `POST` is a method
/// [`crate::google::transport::HttpMethod`] does not yet express.
#[must_use]
pub fn body(parameters: &[(String, String)]) -> String {
    crate::form::encode_body(parameters)
}

/// The `Content-Type` a token request carries.
///
/// Delegated to the codec's own constant rather than restated: RFC 6749 §4.1.3 names one media type for this
/// body, and a second string here is a second thing to keep in step with it.
#[must_use]
pub const fn content_type() -> &'static str {
    crate::form::CONTENT_TYPE
}

/// Performs one token request and maps the exchange onto [`TokenRequestOutcome`].
///
/// # This is what makes `TokenRequestOutcome` reachable
///
/// That type separates `NeverSent` (safe to retry — nothing reached the provider) from `SentAnswerUnknown`
/// (**not** safe — RFC 9700 §4.2.4: a retry of a lost-answer request may get `invalid_grant` *and destroy a
/// working grant the first attempt issued*). It was written with a doc calling the distinction consequential
/// and, until this function existed, **nothing produced either variant**: every construction site was a test.
/// The mapping is one line, and that line is the whole reason the type exists.
///
/// The two directions are not symmetric, which is why the mapping is a wildcard-free consequence of
/// [`TransportFailure::may_have_reached_the_provider`] rather than a `match` on variants here: a `Connect` or a
/// `Refused` is certain, and a `Send`, a `Timeout`, or an unreadable body may have been written.
///
/// # Why an unusable answer is an `Err` and a refusal is an `Ok`
///
/// [`parse_answer`] returns `Err` for a body that is not a grant or refusal and for a grant this platform
/// cannot use. Those are **outcomes that could not be established**, which is a different thing from the
/// provider answering "no" — and `TokenRequestOutcome::Refused` is the latter. So the split is the same one
/// `parse_answer` already draws, surfaced rather than collapsed: a caller that treats every non-answer as a
/// refusal would send a user to a consent screen when the real fault is a body it cannot read.
///
/// # Errors
///
/// Returns [`TokenRequestError`] as [`parse_answer`] does, plus [`TokenRequestError::Body`] for an answer that
/// arrived but could not be read. **A transport failure is never an `Err`**: it is the ambiguity
/// [`TokenRequestOutcome`] exists to express, and returning it here would lose the retry-safety distinction.
pub async fn exchange(
    transport: &dyn crate::google::transport::GoogleTransport,
    request: &crate::google::request::FormRequest,
    refresh_reference: Option<jarvis_core::SecretRef>,
    previous_scopes: &[String],
    requested_scopes: &[String],
) -> Result<TokenRequestOutcome, TokenRequestError> {
    let answer = match send_and_parse(transport, request).await {
        Ok(answer) => answer,
        // **The retry-safety branch `TokenRequestOutcome` exists for**, and the unreadable-body case beside it:
        // the first is the ambiguity, the second is a provider answer this client could not understand.
        Err(ExchangeFailure::Transport(failure)) => {
            return Ok(classify_transport_failure(failure));
        }
        Err(ExchangeFailure::Read(error)) => return Err(error),
    };
    match answer {
        TokenEndpointAnswer::Refused(failure) => Ok(TokenRequestOutcome::Refused(failure)),
        TokenEndpointAnswer::Granted(granted) => {
            // The grant was parsed, so a token set exists; `token_set`'s `Err` is the *unusable* case and is
            // mapped rather than folded into a refusal, because "the provider granted something I cannot use" is
            // a statement by the server while a refusal is a decision it made — and a caller must not send a
            // user to a consent screen for the first.
            let set = TokenEndpointAnswer::Granted(granted)
                .token_set(refresh_reference, previous_scopes, requested_scopes)
                .map_err(unusable_grant)?;
            match set {
                Some(set) => Ok(TokenRequestOutcome::Answered(Box::new(set))),
                // Unreachable for a `Granted` answer, since `token_set` is `Some` whenever `response()` is.
                // Reported rather than panicked on, because `expect` is denied in this crate.
                None => Err(TokenRequestError::Body {
                    reason: "a granted answer produced no token set",
                }),
            }
        }
    }
}

/// Sends one form `POST` and parses its answer, keeping the two failure kinds apart.
///
/// The join both producers share, so the exchange and the refresh cannot diverge in how they send or how they
/// read — the failure `ADR-0069` records for two halves that were never driven together. The two kinds are kept
/// separate because each caller maps them into **its own** vocabulary and the mappings differ: a transport
/// failure becomes `SentAnswerUnknown`/`NeverSent` for a code exchange but `Transient` for a refresh, and a
/// response that cannot be read is an `Err` in both but for different reasons stated at each call site.
async fn send_and_parse(
    transport: &dyn crate::google::transport::GoogleTransport,
    request: &crate::google::request::FormRequest,
) -> Result<TokenEndpointAnswer, ExchangeFailure> {
    let response = transport
        .send_form(request)
        .await
        .map_err(ExchangeFailure::Transport)?;
    parse_answer(response.status, &response.body).map_err(ExchangeFailure::Read)
}

/// Why a token request produced no answer to classify.
///
/// Two variants rather than one because the remedies differ and **only one of them is about the network**: a
/// transport failure is a fact about whether the request was written, while an unreadable body means the
/// provider answered and this client could not understand it. Collapsing them would lose that, and the callers
/// map them differently — the exchange treats an unreadable body as an error and a transport failure as the
/// retry-safety distinction the type exists to make.
enum ExchangeFailure {
    /// The exchange produced no provider answer.
    Transport(crate::google::transport::TransportFailure),
    /// The provider answered and the answer could not be read.
    Read(TokenRequestError),
}

/// Classifies a transport failure into the two cases that are indistinguishable from the request side.
///
/// **This single branch is what `TokenRequestOutcome` exists for.** RFC 9700 §4.2.4 makes a retry of a
/// lost-answer request able to get `invalid_grant` and revoke the tokens the first attempt issued, so
/// "nothing was sent" and "it was sent and nothing came back" must not be one value. The predicate is the
/// port's own (`may_have_reached_the_provider`) rather than a `match` on variants here, so a new
/// [`crate::google::transport::TransportFailure`] variant is classified by the port that owns the meaning.
fn classify_transport_failure(
    failure: crate::google::transport::TransportFailure,
) -> TokenRequestOutcome {
    if failure.may_have_reached_the_provider() {
        TokenRequestOutcome::SentAnswerUnknown
    } else {
        // `NeverSent` holds a `&'static str` so the variant cannot own allocated text, and the failure exposes
        // its own bounded static reason rather than requiring a `Display` that would allocate.
        TokenRequestOutcome::NeverSent {
            reason: failure.reason(),
        }
    }
}

/// Maps `TokenSet::from_response`'s refusal onto this module's own error.
///
/// One function rather than a closure at each call site, so the reason text appears once and the two paths
/// cannot describe the same rejection differently.
fn unusable_grant(_: crate::auth::AuthError) -> TokenRequestError {
    TokenRequestError::UnusableGrant {
        reason: "the grant's token type or lifetime is one this platform will not accept",
    }
}

/// Performs one **refresh** and classifies it, which is what finally produces a [`RefreshExchange`] outside a
/// test.
///
/// # Why this exists, and why the outcome type needed it
///
/// [`crate::token::RefreshExchange`] and `TokenEndpointAnswer::classify_refresh` were written several rounds
/// ago with docs describing RFC 9700 §4.14.2's replay detection as the point of the whole thing — and
/// **nothing produced either**: `refresh` and `classify_refresh` had no caller outside their tests. That is the
/// same defect `ADR-0073` records for `TokenRequestOutcome`, found one layer over in the same module, and it is
/// why this function exists rather than waiting for the composition root.
///
/// # Why the transport failure is `Transient` and not its own variant
///
/// [`crate::auth::RefreshOutcome`] has five variants and **none means "the request may have been written"**.
/// That is a deliberate difference from the code exchange, and the reason is the consequence: a refresh
/// presents the **stored** refresh token, so if a rotation silently landed the stored reference is already
/// invalid and the next attempt fails with `invalid_grant` — which this function then reports as `Expired` or
/// `Revoked`, i.e. as `needs_user`. So the ambiguous case is **self-correcting**: it costs one extra call and
/// then asks the user, where retrying a lost *code* exchange could revoke tokens. Reporting it as `Transient`
/// is honest about the retry being safe.
///
/// **The vocabulary gap is a recorded limit, not a claim.** A rotation that arrived unread is invisible here
/// until the next attempt; a caller that needed to know sooner would need a variant `RefreshOutcome` does not
/// have, which is a change to a shared type rather than a fix to this function.
///
/// # Errors
///
/// As [`parse_answer`]. **A transport failure is never an `Err`**: it is classified into an outcome, because
/// "nothing came back" is a fact about the exchange rather than a fault in reading it.
///
/// # What this does not do
///
/// It does not mint the refresh token, and it does not store a rotated one. The caller holds the stored material
/// and owns the secret store, so it supplies the previous scopes and receives the new reference in the outcome —
/// the division `TokenSet::from_response` already records.
pub async fn refresh_with(
    transport: &dyn crate::google::transport::GoogleTransport,
    request: &crate::google::request::FormRequest,
    new_reference: Option<jarvis_core::SecretRef>,
    previous_scopes: &[String],
    requested_scopes: &[String],
    vendor_says_revoked: bool,
) -> Result<crate::token::RefreshExchange, TokenRequestError> {
    let answer = match send_and_parse(transport, request).await {
        Ok(answer) => answer,
        // `Transport` → the transient outcome the doc justifies; `Read` → the error `parse_answer` produced.
        Err(ExchangeFailure::Transport(_)) => {
            return Ok(crate::token::RefreshExchange::classify(
                None,
                None,
                new_reference,
                previous_scopes,
                requested_scopes,
                vendor_says_revoked,
            ));
        }
        Err(ExchangeFailure::Read(error)) => return Err(error),
    };
    Ok(answer.classify_refresh(
        new_reference,
        previous_scopes,
        requested_scopes,
        vendor_says_revoked,
    ))
}

/// Reads a token endpoint's answer.
///
/// # The rule, and why the body decides
///
/// RFC 6749 §5.1: a successful response contains `access_token` and `token_type`, and §5.2 makes a failure an
/// `error` parameter with the token parameters omitted. So **the presence of `error`** is what makes an answer
/// a refusal, and the HTTP status is *not*: a `400` without an `error` is a proxy's page or a misrouted
/// request, and calling it "the server refused" would attribute a decision to a server that never made one.
///
/// The status is still read, for the one thing it does say — whether the provider is unwell. A `5xx` marks the
/// refusal transient, which is knowledge the transport has and the protocol's codes do not carry, because RFC
/// 6749 §5.2's values describe the *request* rather than the server's health.
///
/// # The access token is read for its presence and never into a value
///
/// This function receives the whole body, so the text is in scope here — and it reports only **that** a token
/// arrived. It does not copy it: an owned `String` would be a second, unredacted copy of the credential with a
/// lifetime, which is the thing `ADR-0061` exists to prevent. An earlier draft did exactly that ("read it into
/// a `String` and dropped it"), which satisfies the letter of the rule and breaks its purpose.
///
/// # Errors
///
/// Returns [`TokenRequestError::Body`] when the body is not a JSON object or describes neither a grant nor a
/// refusal, and [`TokenRequestError::UnusableGrant`] when a grant's `token_type` or `expires_in` is one this
/// platform will not accept.
pub fn parse_answer(status: u16, body: &str) -> Result<TokenEndpointAnswer, TokenRequestError> {
    let value: Value = serde_json::from_str(body).map_err(|_| TokenRequestError::Body {
        reason: "a token response must be a JSON object",
    })?;
    let object = value.as_object().ok_or(TokenRequestError::Body {
        reason: "a token response must be a JSON object",
    })?;

    // A refusal is decided by the parameter's PRESENCE, not by its value: RFC 6749 §5.2 makes it required in an
    // error response, so an empty `error` is a malformed refusal rather than a grant that mentions the word.
    if let Some(error) = object.get("error") {
        let code = error.as_str().unwrap_or_default().to_owned();
        // The description is read and then **not used for any classification**. It is prose, and `P3-008c`'s
        // rule is that a decision must not come from message text — so it is carried only because a diagnostic
        // that cannot say what the provider wrote is one an operator cannot act on.
        let description = object
            .get("error_description")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned);
        return Ok(TokenEndpointAnswer::Refused(TokenEndpointFailure::new(
            code,
            description,
            // The only thing the status contributes: an outage is not a verdict on the request.
            (500..=599).contains(&status),
        )));
    }

    // Presence, not the bytes. `.is_some_and` keeps the text inside this expression and produces a `bool`.
    let present = |name: &str| -> bool {
        object
            .get(name)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    };
    let text_of = |name: &str| -> Option<String> {
        object
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };

    if !present("access_token") {
        return Err(TokenRequestError::Body {
            reason: "a token response must carry a non-empty `access_token`, or an `error` naming why not",
        });
    }
    let response = TokenResponse::new(
        text_of("token_type"),
        object.get("expires_in").and_then(Value::as_i64),
        text_of("scope"),
        present(REFRESH_TOKEN_PARAMETER),
        present("id_token"),
    );
    // The grant is checked here rather than left to `TokenSet::from_response`, so a caller that only wants to
    // know whether the exchange worked does not have to know the check lives in another module. Both call the
    // same predicates, so they cannot disagree — and both are asserted on the same values.
    if !response.token_type_is_usable() {
        return Err(TokenRequestError::UnusableGrant {
            reason: "the `token_type` is not `Bearer`, which RFC 6750 §2.1 defines as the only scheme OAuth 2.0 \
                     uses for a resource request",
        });
    }
    if let Some(seconds) = response.lifetime_seconds()
        && seconds > MAX_ACCESS_TOKEN_SECONDS
    {
        return Err(TokenRequestError::UnusableGrant {
            reason: "the declared access-token lifetime exceeds what this platform accepts, which is \
                     either a server defect or a response that is not a token",
        });
    }
    // The refresh material is wrapped immediately, so its plaintext exists for the shortest possible window and
    // never as a bare `String` that a later `Debug` could reach. `text_of` returns an owned copy for one
    // statement, and `Secret::new` consumes it — the alternative is a closure that would have to return the
    // owned value anyway, so the window is the same and the code is clearer.
    let refresh_token = text_of(REFRESH_TOKEN_PARAMETER)
        .map(|value| Secret::new(REFRESH_TOKEN_PARAMETER, value, MAX_REFRESH_TOKEN_CHARS))
        .transpose()?;
    Ok(TokenEndpointAnswer::Granted(Granted {
        response,
        refresh_token,
    }))
}

/// Compares two scope lists using the project's single comparison.
///
/// Offered so a caller does not reach for a second implementation: `ScopeSetChange::between` is the one place a
/// loss takes precedence over a gain.
#[must_use]
pub fn scope_change(previous: &[String], granted: &[String]) -> ScopeSetChange {
    ScopeSetChange::between(previous, granted)
}

/// Splits a `scope` parameter using the shared splitter.
#[must_use]
pub fn granted_scopes(scope: &str) -> Vec<String> {
    split_scope(scope)
}

#[cfg(test)]
#[path = "token_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "token_exchange_tests.rs"]
mod exchange_tests;
