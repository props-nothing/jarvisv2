//! The access-token boundary: material a transport may use and nothing else may see.
//!
//! # Why this type exists, given that `HttpRequest` already has no credential field
//!
//! `ADR-0060` made a credential unrepresentable *in a URL* by giving [`crate::google::request::HttpRequest`] no
//! field to hold one. That closes one route. It does not close the others, and they are the routes that
//! actually leak credentials in practice:
//!
//! - **a log line** — `tracing::debug!("calling {} with {:?}", url, headers)` prints the header map;
//! - **a diagnostics record** — `DiagnosticField::ProviderRequestId` and `RetryClass` exist precisely because
//!   a provider's error text is *rendered*, and a struct that derives `Debug` renders whatever it holds;
//! - **an error message** — a transport that wraps its own failure in `format!("{request:?}")`;
//! - **a serialized payload** — anything that puts a request into a durable row or a wire DTO.
//!
//! Every one of those is a `Debug`, `Serialize`, or `Display` impl. So this module's central decision is that
//! [`AccessToken`] implements **none** of them: `Debug` is hand-written to `[REDACTED]` and the type is
//! deliberately not `Serialize`. The `compile_fail` doctest below is the assertion, so a later
//! `#[derive(Serialize)]` breaks the build rather than silently enabling a leak.
//!
//! # Why the token is not a plain `String` passed to a send function
//!
//! A `String` argument is `Debug`-printable by whoever holds it, and a `&str` borrows out of a caller's
//! storage that the caller may also log. Handing over this type instead means the *transport* is the only
//! place the bytes are reachable, through [`AccessToken::with_exposed`] — a name chosen so that a reviewer
//! reading a call site sees an exposure happening.
//!
//! # What this is not
//!
//! - **Not a secret *store*.** It holds material already obtained from one. Where a refresh token lives is
//!   `P5-002`'s `TokenSet` and a `SecretRef`, and this type does not read it.
//! - **Not zeroized on drop.** `zeroize` is not a dependency, and adding one for this is a decision rather
//!   than a detail. The residual is recorded in the limits of `ADR-0061` rather than implied.
//! - **Not proof of anything.** Where the material came from, and whether it is valid, are the transport's and
//!   the provider's questions. A token that *looks* well-formed proves nothing.

use std::fmt;

use jarvis_core::SecretRef;

/// The scheme every Google API credential is presented with.
///
/// The same constant the request module states, repeated here deliberately: the *shape* of an authorization
/// header belongs to the request, and the *material* belongs to this type. Keeping the two constants adjacent
/// in meaning makes it obvious to a reader that they must agree — and they are asserted equal by a test.
pub const BEARER_SCHEME: &str = "Bearer";

/// The shortest access-token value accepted, in characters.
///
/// A floor rather than a guess about entropy: Google's own sample token is far longer, and a value short
/// enough to be a copy-paste accident (`"abc"`, a client id, a project number) is refused at construction so
/// the mistake surfaces where it was made rather than as a provider auth error — the reasoning
/// `P5-004`'s `ApiKey::new` records for rejecting a pasted URL.
pub const MIN_ACCESS_TOKEN_CHARS: usize = 20;

/// The longest access-token value accepted, in characters.
///
/// A bound on a value that becomes a header, because an unbounded one is an unbounded request — the same rule
/// `HttpRequest` applies to a page token.
pub const MAX_ACCESS_TOKEN_CHARS: usize = 4_096;

/// Why an access token was refused.
///
/// Each variant names **what is wrong with the value** without ever including it, which is the one thing an
/// error about a credential must not do: an error message reaches a log, and an error that renders the token
/// it refused has leaked the value it was protecting.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AccessTokenError {
    /// The value is empty.
    #[error("an access token may not be empty")]
    Empty,
    /// The value is shorter than any real token.
    #[error("an access token must be at least 20 characters; this is probably not one")]
    TooShort,
    /// The value is longer than any header should carry.
    #[error("an access token may be at most 4096 characters")]
    TooLong,
    /// The value holds whitespace, which is what a copied header looks like.
    ///
    /// The specific check that earns its place: an operator or a script pasting `Authorization: Bearer abc…`
    /// supplies a value containing a space, and the resulting provider error is a generic auth failure that
    /// sends a reader to debug the credential's *validity* rather than its *shape*. `P5-004` records the same
    /// reasoning for an API key.
    #[error(
        "an access token may not contain whitespace; if you copied an `Authorization` header, supply only \
         the token value"
    )]
    ContainsWhitespace,
    /// The value holds a control character, which would forge a header or a log line.
    #[error("an access token may not contain a control character")]
    ContainsControl,
    /// The value already carries a scheme prefix, which the transport adds itself.
    ///
    /// Distinct from [`Self::ContainsWhitespace`] because the remedy differs — this one is "remove `Bearer`",
    /// the other is "remove the space" — and because a caller that supplied the scheme would otherwise get a
    /// header reading `Authorization: Bearer Bearer <token>`.
    #[error("an access token must not include the `Bearer` scheme; the transport adds it")]
    IncludesScheme,
}

/// An access token, held in a type that cannot render or serialize it.
///
/// Constructing one is the only way to obtain material for a request, and the only accessor is
/// [`Self::with_exposed`], so every use is a visible exposure at the call site.
///
/// See the module documentation for why `Debug` is hand-written and `Serialize` is absent. The absence is
/// asserted here rather than described, so a later `#[derive(Serialize)]` is a **compile failure**:
///
/// ```compile_fail
/// # use jarvis_connectors::google::credential::AccessToken;
/// let token = AccessToken::new("ya29.a0AfH6SMBsecretvalue1234567890")?;
/// let _json = serde_json::to_string(&token)?;
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct AccessToken {
    /// Never rendered. The field is private and no accessor returns it directly.
    value: String,
    /// Where the *material* came from, so a diagnostic can say which credential failed.
    ///
    /// A [`SecretRef`] is metadata that locates a secret, so recording it beside the token lets a failure be
    /// attributed to "the stored refresh exchange for account X" without the token itself being part of any
    /// record. This is the one field a diagnostic may print.
    origin: Option<SecretRef>,
}

impl AccessToken {
    /// Records an access token.
    ///
    /// # Errors
    ///
    /// Returns [`AccessTokenError`] when the value is empty, shorter than [`MIN_ACCESS_TOKEN_CHARS`], longer
    /// than [`MAX_ACCESS_TOKEN_CHARS`], holds whitespace or a control character, or already carries the
    /// `Bearer` scheme. None of those messages contains the value.
    pub fn new(value: impl Into<String>) -> Result<Self, AccessTokenError> {
        let value = value.into();
        // Checked in this order deliberately: the scheme check produces the most actionable message, and the
        // whitespace check would fire first on a pasted header and report "contains whitespace" — true,
        // useless, and it sends a reader hunting an invisible character. `P5-004` records the same ordering
        // decision for an API key.
        if value.starts_with(BEARER_SCHEME) || value.starts_with("bearer ") {
            return Err(AccessTokenError::IncludesScheme);
        }
        if value.is_empty() {
            return Err(AccessTokenError::Empty);
        }
        if value.chars().any(char::is_control) {
            return Err(AccessTokenError::ContainsControl);
        }
        if value.chars().any(char::is_whitespace) {
            return Err(AccessTokenError::ContainsWhitespace);
        }
        if value.chars().count() < MIN_ACCESS_TOKEN_CHARS {
            return Err(AccessTokenError::TooShort);
        }
        if value.chars().count() > MAX_ACCESS_TOKEN_CHARS {
            return Err(AccessTokenError::TooLong);
        }
        Ok(Self {
            value,
            origin: None,
        })
    }

    /// Records where the material came from, for attribution in a diagnostic.
    #[must_use]
    pub fn with_origin(mut self, origin: SecretRef) -> Self {
        self.origin = Some(origin);
        self
    }

    /// Returns the metadata that locates the secret this material came from, when it is known.
    ///
    /// The accessor a **diagnostic** uses: a `SecretRef` is metadata, its `Debug` renders its locator as
    /// `[REDACTED]`, and it names which credential failed without naming the credential.
    #[must_use]
    pub fn origin(&self) -> Option<&SecretRef> {
        self.origin.as_ref()
    }

    /// Runs a closure with the token's bytes.
    ///
    /// **The only way to read the value**, and named so that a reviewer notices the exposure rather than
    /// reading a field access. A closure rather than a `&str` return so the borrow cannot outlive the call —
    /// which is what stops a caller from storing a reference where a later `Debug` could reach it.
    ///
    /// The closure is deliberately **not** given the `AccessToken`, only the text: a closure handed the token
    /// could render *that*, and the point of the shape is that reaching the bytes requires the caller to write
    /// out the word `exposed`.
    pub fn with_exposed<T>(&self, use_token: impl FnOnce(&str) -> T) -> T {
        use_token(&self.value)
    }

    /// Returns the length of the token, in characters.
    ///
    /// A **length, never the value**, and it exists for the same reason `SecretRef`'s rendering does: a
    /// diagnostic that can report "the stored token is 32 characters" answers "is this the right credential?"
    /// without the credential being part of the record. It is also how a test can assert two tokens differ
    /// without either being printed.
    #[must_use]
    pub fn char_len(&self) -> usize {
        self.value.chars().count()
    }

    /// Renders the `Authorization` header value, as a new `String`.
    ///
    /// The one place the bytes and the scheme meet, so a transport does not construct the header itself — a
    /// transport that did would be a second implementation of `Bearer ` plus a value, which is where a
    /// missing space or a doubled scheme comes from.
    ///
    /// **This returns material.** The result is a credential-bearing `String` with no protection, so it is
    /// named `authorization_header_value` rather than `header` to make its content obvious at every call site.
    #[must_use]
    pub fn authorization_header_value(&self) -> String {
        format!("{BEARER_SCHEME} {}", self.value)
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A hand-written `Debug` rather than a derive, and this is the module's central control: a derived
        // `Debug` renders whatever the struct holds, so one `{:?}` anywhere — a log, an error, a test failure
        // message — prints the token. `SecretRef`'s hand-written `Debug` records the same reasoning.
        //
        // The length is reported because it is not the value and it is what a diagnostic needs: two tokens of
        // different lengths are visibly different credentials.
        write!(
            formatter,
            "AccessToken {{ value: [REDACTED], chars: {} }}",
            self.value.chars().count()
        )
    }
}

/// The authorization header NAME, so the request and this type cannot disagree about it.
///
/// A constant rather than a value, because the name is not a secret; asserted equal to
/// [`crate::google::request::AUTHORIZATION_HEADER`] by a test, so the two modules state one name.
pub const AUTHORIZATION_HEADER: &str = "Authorization";

#[cfg(test)]
#[path = "credential_tests.rs"]
mod tests;
