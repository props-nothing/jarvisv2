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
//!
//! # What is not built
//!
//! No request is sent. There is no transport for a form `POST`, so the caller owns the exchange and this module
//! owns only what is verifiable without a socket.

use serde_json::Value;

use crate::auth::{AuthError, RefreshOutcome, ScopeSetChange};
use crate::token::{
    MAX_ACCESS_TOKEN_SECONDS, TokenEndpointFailure, TokenResponse, TokenSet, split_scope,
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
    #[must_use]
    pub fn classify_refresh(
        &self,
        new_reference: Option<jarvis_core::SecretRef>,
        previous_scopes: &[String],
        requested_scopes: &[String],
        vendor_says_revoked: bool,
    ) -> crate::token::RefreshExchange {
        // Rotation is decided by whether refresh material ARRIVED, never by whether the caller stored it: a
        // caller that failed to store one has a defect of its own, and reporting `Refreshed` would hide the
        // half of the exchange that makes replay detectable (RFC 9700 §4.14.2). The clone is of a `SecretRef`,
        // which is metadata — a locator, not a secret — so copying it is not a credential copy.
        let rotated_reference = match self.refresh_token() {
            Some(_) => new_reference.clone(),
            None => None,
        };
        crate::token::RefreshExchange {
            outcome: refresh_outcome(self, vendor_says_revoked),
            token_set: self
                .token_set(new_reference, previous_scopes, requested_scopes)
                .ok()
                .flatten(),
            rotated_reference,
        }
    }
}

/// Maps an answer onto the refresh vocabulary.
///
/// A granted response is `Rotated` or `Refreshed` by whether refresh material arrived; a refusal is `Expired`,
/// `Revoked`, or `Transient` by the rule `RefreshExchange::classify` applies. A function so `classify_refresh`
/// reads as the decisions it makes rather than as a nested `match` — and so the *ordering* below is visible in
/// one place.
fn refresh_outcome(answer: &TokenEndpointAnswer, vendor_says_revoked: bool) -> RefreshOutcome {
    match answer {
        TokenEndpointAnswer::Granted(granted) => {
            if granted.refresh_token.is_some() {
                RefreshOutcome::Rotated
            } else {
                // Google's documented refresh does **not** return a new refresh token, so this is the expected
                // outcome for this provider rather than a degradation. Rotation is still detected, because a
                // provider that started rotating would otherwise go unnoticed.
                RefreshOutcome::Refreshed
            }
        }
        TokenEndpointAnswer::Refused(failure) => {
            // The transient check is FIRST, and the ordering is load-bearing: a provider behind a proxy can
            // answer a 503 through the protocol's own error channel, and reading the code first would send a
            // user to a consent screen during an outage. `RefreshExchange::classify` records the same ordering
            // for the same reason.
            if failure.transient {
                RefreshOutcome::Transient
            } else if failure.requires_reauth() {
                if vendor_says_revoked {
                    RefreshOutcome::Revoked
                } else {
                    // `invalid_grant` covers "invalid, expired, revoked, does not match the redirection URI,
                    // or was issued to another client" (RFC 6749 §5.2), and the protocol does not say which. So
                    // the caller's vendor knowledge decides, and the default is the one that asks the user to
                    // authorize again rather than the one that tells them their access was withdrawn.
                    RefreshOutcome::Expired
                }
            } else {
                // `invalid_client`/`unauthorized_client` mean the *client* is misconfigured, so sending the
                // user to a consent screen lands on the same failure. Reported as transient so
                // `is_safe_to_retry` is false and `needs_user` is false: something is wrong that the user
                // cannot fix.
                RefreshOutcome::Transient
            }
        }
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

    /// Returns the parameters every token request carries.
    ///
    /// Both are percent-encoded. Google's own identifiers are URL-safe, so this looks redundant — and it is not:
    /// the encoding is what makes a value that *is* unusual harmless rather than what makes a typical value
    /// work. A redirect URI always contains `://` and `/`, so it is never in the unreserved set.
    fn parameters(&self) -> Vec<(String, String)> {
        vec![
            (
                CLIENT_ID_PARAMETER.to_owned(),
                crate::google::request::percent_encode(&self.client_id),
            ),
            (
                REDIRECT_URI_PARAMETER.to_owned(),
                crate::google::request::percent_encode(&self.redirect_uri),
            ),
        ]
    }

    /// Returns the identity parameters, for a caller assembling a request of its own.
    #[must_use]
    pub fn public_parameters(&self) -> Vec<(String, String)> {
        self.parameters()
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
