//! Signing in to a Google account, and keeping the sign-in alive.
//!
//! The owner creates a Desktop-app OAuth client in their own Google Cloud project once (`docs/user/google.md`), puts its id and secret
//! in Settings, and presses **Sign in with Google**. This module is everything behind that button:
//!
//! 1. [`GoogleAccount::begin`] opens an authorization transaction (PKCE `S256`, a random `state`, a loopback redirect on the
//!    daemon's own port) and returns the Google address to open. At most one transaction is pending; a second supersedes the first.
//! 2. Google redirects the browser to `http://127.0.0.1:<port>/oauth/google/callback`. [`GoogleAccount::complete`] validates the
//!    answer with the connector crate's `AuthorizationTransaction::consume` (state, redirect, provider error) and exchanges the code.
//! 3. The refresh token is stored in a private file beside the configuration; the access token lives only in memory and is refreshed
//!    on demand ([`GoogleAccount::access_token`]).
//! 4. [`GoogleAccount::disconnect`] revokes the grant at Google and deletes the file.
//!
//! The protocol walk (PKCE, state, single-use transaction, loopback-only redirect) is the connector crate's (`P5-002`); this adds the
//! two HTTP calls the crate deliberately does not make, and the storage. Sources: `docs/research/integrations/google.md` and
//! `oauth2-pkce-native-apps.md`.
//!
//! # What never leaves this module
//!
//! A token, a code, a client secret or a verifier appears in no log line, no error text and no reply. Errors are a fixed set of
//! sentences ([`GoogleError`]); Google's own error text is never echoed.

use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use jarvis_connectors::google::{GoogleConnector, SCOPE_CALENDAR_READONLY, SCOPE_GMAIL_READONLY};

use jarvis_connectors::{
    AuthFlow, AuthMethod, AuthorizationTransaction, Callback, LoopbackHost, LoopbackRedirect,
    PkceMethod, PkceVerifier, SecretValue,
};
use jarvis_core::{SystemClock, UtcTimestamp};
use jarvis_storage::{AppPaths, ConfigStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::sync::Mutex;

/// The path Google redirects the browser to, on the daemon's own port.
pub const CALLBACK_PATH: &str = "/oauth/google/callback";

/// How long an unanswered sign-in stays valid.
const PENDING_SECONDS: i128 = 600;
/// An access token this close to expiry is refreshed first.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const TOKEN_FILE: &str = "google.token";
const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// A form body, percent-encoded by the connector crate's own encoder.
fn form_body(fields: &[(&str, String)]) -> String {
    let owned: Vec<(String, String)> = fields
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect();
    jarvis_connectors::form::encode_body(&owned)
}

/// The scopes always asked for: read mail and read the calendar.
pub const SCOPES: [&str; 2] = [SCOPE_GMAIL_READONLY, SCOPE_CALENDAR_READONLY];

/// Sending mail (and nothing else of Gmail's write surface), asked for only when the owner turns actions on.
pub const SCOPE_GMAIL_SEND: &str = "https://www.googleapis.com/auth/gmail.send";
/// Creating and changing calendar events, asked for only when the owner turns actions on.
pub const SCOPE_CALENDAR_EVENTS: &str = "https://www.googleapis.com/auth/calendar.events";

/// Why a Google step failed, in sentences safe to show.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum GoogleError {
    /// No client id is set, so Google sign-in is not configured.
    #[error("Google is not set up: add a Google client id in Settings, Google")]
    NotConfigured,
    /// Nobody is signed in.
    #[error("Google is not connected: the owner can sign in from Settings, Google")]
    NotConnected,
    /// The stored sign-in no longer works (revoked or expired); it was removed.
    #[error("the Google sign-in expired or was revoked; sign in again from Settings, Google")]
    SignInExpired,
    /// The browser answer did not belong to a sign-in this daemon started, or was malformed.
    #[error("that Google sign-in answer was refused: it did not match a sign-in started here")]
    Refused,
    /// Google reported an error to the browser (for example the owner pressed cancel).
    #[error("Google did not complete the sign-in (it was cancelled or refused)")]
    ProviderError,
    /// Google answered the code exchange or refresh with a failure.
    #[error("Google refused the sign-in exchange; check the client id and secret in Settings")]
    ExchangeFailed,
    /// The owner did not grant every permission asked for.
    #[error("not every permission was granted; sign in again and tick both mail and calendar")]
    ScopesMissing,
    /// Google could not be reached or answered something unintelligible.
    #[error("Google could not be reached or answered unexpectedly")]
    Unreachable,
    /// The sign-in could not be stored or removed.
    #[error("the Google sign-in could not be saved on this machine")]
    Storage,
    /// The system could not make a random value.
    #[error("the system could not produce a secure random value")]
    Random,
}

/// The addresses used, replaceable so tests can talk to a local fixture.
#[derive(Clone, Debug)]
pub struct Endpoints {
    /// Where the browser is sent.
    pub authorization: String,
    /// The token endpoint.
    pub token: String,
    /// The revocation endpoint.
    pub revoke: String,
    /// The Gmail API base, without a trailing slash.
    pub gmail: String,
    /// The Calendar API base, without a trailing slash.
    pub calendar: String,
}

impl Endpoints {
    /// Google's own addresses.
    #[must_use]
    pub fn google() -> Self {
        Self {
            authorization: GoogleConnector::authorization_endpoint().to_owned(),
            token: GoogleConnector::token_endpoint().to_owned(),
            revoke: GoogleConnector::revocation_endpoint().to_owned(),
            gmail: "https://gmail.googleapis.com/gmail/v1".to_owned(),
            calendar: "https://www.googleapis.com/calendar/v3".to_owned(),
        }
    }
}

/// What a person may be told about the account.
// Independent facts about the account shown as they are on the wire; folding them into a state enum would only make the console translate it back.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Status {
    /// A client id is configured.
    pub configured: bool,
    /// Someone is signed in.
    pub connected: bool,
    /// The signed-in mailbox address.
    pub email: Option<String>,
    /// A sign-in was started and has not been answered yet.
    pub pending: bool,
    /// The owner turned on sending mail and creating events.
    pub actions: bool,
    /// The signed-in grant covers sending and event creation.
    pub can_act: bool,
}

#[derive(Deserialize, Serialize)]
struct Stored {
    refresh_token: String,
    email: String,
    scopes: Vec<String>,
}

struct Pending {
    transaction: AuthorizationTransaction,
    verifier: PkceVerifier,
    /// What this sign-in asked for, so the answer is held to it.
    scopes: Vec<String>,
}

#[derive(Default)]
struct Memory {
    pending: Option<Pending>,
}

struct Cached {
    token: String,
    expires: Instant,
}

/// The signed-in Google account, if any.
pub struct GoogleAccount {
    paths: AppPaths,
    client: reqwest::Client,
    endpoints: Endpoints,
    memory: StdMutex<Memory>,
    access: Mutex<Option<Cached>>,
}

impl std::fmt::Debug for GoogleAccount {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GoogleAccount")
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: Option<String>,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    scope: Option<String>,
    token_type: Option<String>,
}

impl GoogleAccount {
    /// Builds the account service over the profile directories, using Google's addresses.
    ///
    /// # Errors
    ///
    /// Returns [`GoogleError::Unreachable`] when the HTTP client cannot be built.
    pub fn new(paths: AppPaths) -> Result<Self, GoogleError> {
        Self::with_endpoints(paths, Endpoints::google())
    }

    /// The one account service of this daemon process, shared by the sign-in routes and the mail and calendar tools so they agree on
    /// who is signed in and on the cached access token (a disconnect must reach the tools at once).
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn shared(paths: &AppPaths) -> Result<std::sync::Arc<Self>, GoogleError> {
        static SHARED: std::sync::OnceLock<std::sync::Arc<GoogleAccount>> =
            std::sync::OnceLock::new();
        if let Some(existing) = SHARED.get() {
            return Ok(std::sync::Arc::clone(existing));
        }
        let created = std::sync::Arc::new(Self::new(paths.clone())?);
        Ok(std::sync::Arc::clone(SHARED.get_or_init(|| created)))
    }

    /// Builds the account service against other addresses (a test fixture).
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn with_endpoints(paths: AppPaths, endpoints: Endpoints) -> Result<Self, GoogleError> {
        // No redirects: a bearer token must never be carried to another host.
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| GoogleError::Unreachable)?;
        Ok(Self {
            paths,
            client,
            endpoints,
            memory: StdMutex::new(Memory::default()),
            access: Mutex::new(None),
        })
    }

    /// The Gmail API base.
    #[must_use]
    pub fn gmail_base(&self) -> &str {
        &self.endpoints.gmail
    }

    /// The Calendar API base.
    #[must_use]
    pub fn calendar_base(&self) -> &str {
        &self.endpoints.calendar
    }

    /// The shared HTTP client (no redirects).
    #[must_use]
    pub const fn http(&self) -> &reqwest::Client {
        &self.client
    }

    /// The scopes a sign-in asks for now: the two read scopes, plus sending and event creation when the owner turned actions on.
    fn wanted_scopes(&self) -> Vec<String> {
        let actions = ConfigStore::from_paths(&self.paths)
            .load()
            .is_ok_and(|loaded| loaded.config().daemon().google_actions_enabled());
        let mut scopes: Vec<String> = SCOPES.iter().map(|scope| (*scope).to_owned()).collect();
        if actions {
            scopes.push(SCOPE_GMAIL_SEND.to_owned());
            scopes.push(SCOPE_CALENDAR_EVENTS.to_owned());
        }
        scopes
    }

    /// Whether the stored sign-in was granted scope. False when nobody is signed in.
    #[must_use]
    pub fn has_scope(&self, scope: &str) -> bool {
        self.read_stored()
            .is_some_and(|stored| stored.scopes.iter().any(|have| have == scope))
    }

    fn token_path(&self) -> PathBuf {
        self.paths.config().join(TOKEN_FILE)
    }

    /// The client id and secret from the saved configuration, read each time so a change applies at once.
    fn credentials(&self) -> Result<(String, Option<String>), GoogleError> {
        let loaded = ConfigStore::from_paths(&self.paths)
            .load()
            .map_err(|_| GoogleError::NotConfigured)?;
        let daemon = loaded.config().daemon();
        let id = daemon
            .google_client_id()
            .ok_or(GoogleError::NotConfigured)?
            .to_owned();
        let secret = daemon
            .google_client_secret_ref()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty());
        Ok((id, secret))
    }

    fn read_stored(&self) -> Option<Stored> {
        let text = std::fs::read_to_string(self.token_path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn write_stored(&self, stored: &Stored) -> Result<(), GoogleError> {
        let path = self.token_path();
        let text = serde_json::to_string(stored).map_err(|_| GoogleError::Storage)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| GoogleError::Storage)?;
        }
        std::fs::write(&path, text).map_err(|_| GoogleError::Storage)?;
        jarvis_storage::secure_private_file(jarvis_storage::PathKind::Config, &path)
            .map_err(|_| GoogleError::Storage)
    }

    /// What can be said about the account right now.
    #[must_use]
    pub fn status(&self) -> Status {
        let stored = self.read_stored();
        let wanted = self.wanted_scopes();
        let pending = self
            .memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .is_some();
        Status {
            configured: self.credentials().is_ok(),
            connected: stored.is_some(),
            email: stored.map(|stored| stored.email),
            pending,
            actions: wanted.len() > SCOPES.len(),
            can_act: self.has_scope(SCOPE_GMAIL_SEND) && self.has_scope(SCOPE_CALENDAR_EVENTS),
        }
    }

    /// Starts a sign-in and returns the Google address to open in a browser.
    ///
    /// `port` is the daemon's own HTTP port, where Google sends the browser back.
    ///
    /// # Errors
    ///
    /// Returns [`GoogleError::NotConfigured`] without a client id, and [`GoogleError::Random`] or [`GoogleError::Refused`] when the
    /// transaction cannot be built.
    pub fn begin(&self, port: u16) -> Result<String, GoogleError> {
        let (client_id, _) = self.credentials()?;
        let flow = AuthFlow::new(
            AuthMethod::OAuthPkce,
            Some(PkceMethod::S256),
            Some(format!("http://127.0.0.1{CALLBACK_PATH}")),
            self.endpoints.authorization.clone(),
        )
        .map_err(|_| GoogleError::Refused)?;
        let redirect = LoopbackRedirect::listening(LoopbackHost::V4, port, CALLBACK_PATH)
            .map_err(|_| GoogleError::Refused)?;
        let verifier = PkceVerifier::generate().map_err(|_| GoogleError::Random)?;
        let state = SecretValue::new(
            PkceVerifier::generate()
                .map_err(|_| GoogleError::Random)?
                .expose(),
        )
        .map_err(|_| GoogleError::Random)?;
        let transaction = AuthorizationTransaction::begin(
            &flow,
            verifier.clone(),
            state,
            None,
            redirect,
            UtcTimestamp::now(&SystemClock),
        )
        .map_err(|_| GoogleError::Refused)?;
        let scopes = self.wanted_scopes();
        let mut parameters = transaction.parameters(&client_id, &scopes);
        // A fresh consent screen, so Google issues a refresh token every time (a repeat sign-in would otherwise return none).
        parameters.push(("prompt", "consent".to_owned()));
        let url = reqwest::Url::parse_with_params(
            &self.endpoints.authorization,
            parameters.iter().map(|(key, value)| (*key, value.as_str())),
        )
        .map_err(|_| GoogleError::Refused)?;
        // Replaces any earlier transaction: two live ones could be matched to the wrong answer.
        self.memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending = Some(Pending {
            transaction,
            verifier,
            scopes,
        });
        Ok(url.to_string())
    }

    /// Finishes a sign-in from the request target Google sent the browser to, and returns the signed-in address.
    ///
    /// Fails closed: no pending sign-in, a different `state`, an error from Google, an expired transaction or a repeated answer
    /// each end in a refusal and store nothing. The transaction is consumed by the first answer, valid or not.
    ///
    /// # Errors
    ///
    /// Returns the [`GoogleError`] describing which step failed.
    pub async fn complete(&self, request_target: &str) -> Result<String, GoogleError> {
        let pending = self
            .memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending
            .take()
            .ok_or(GoogleError::Refused)?;
        if pending
            .transaction
            .is_expired(UtcTimestamp::now(&SystemClock), PENDING_SECONDS)
        {
            return Err(GoogleError::Refused);
        }
        let callback =
            Callback::from_request_target(request_target).map_err(|_| GoogleError::Refused)?;
        let grant = pending
            .transaction
            .consume(&callback)
            .map_err(|refusal| match refusal {
                jarvis_connectors::AuthRefusal::ProviderError { .. } => GoogleError::ProviderError,
                _ => GoogleError::Refused,
            })?;
        let (client_id, secret) = self.credentials()?;
        let mut form = vec![
            ("grant_type", "authorization_code".to_owned()),
            ("code", grant.code.expose().to_owned()),
            ("client_id", client_id),
            ("code_verifier", pending.verifier.expose().to_owned()),
            ("redirect_uri", grant.redirect.as_uri()),
        ];
        if let Some(secret) = secret {
            form.push(("client_secret", secret));
        }
        let reply = self.token_request(&form).await?;
        let (Some(access), Some(refresh)) =
            (reply.access_token.clone(), reply.refresh_token.clone())
        else {
            return Err(GoogleError::ExchangeFailed);
        };
        if reply
            .token_type
            .as_deref()
            .is_some_and(|kind| !kind.eq_ignore_ascii_case("bearer"))
        {
            return Err(GoogleError::ExchangeFailed);
        }
        let granted: Vec<String> = reply
            .scope
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if !pending
            .scopes
            .iter()
            .all(|wanted| granted.iter().any(|have| have == wanted))
        {
            // Not stored; the grant Google just made is withdrawn so nothing lingers.
            self.revoke(&refresh).await;
            return Err(GoogleError::ScopesMissing);
        }
        let email = self.profile_email(&access).await?;
        self.write_stored(&Stored {
            refresh_token: refresh,
            email: email.clone(),
            scopes: granted,
        })?;
        *self.access.lock().await = Some(Cached {
            token: access,
            expires: Instant::now() + Duration::from_secs(reply.expires_in.unwrap_or(3000)),
        });
        Ok(email)
    }

    async fn token_request(&self, form: &[(&str, String)]) -> Result<TokenReply, GoogleError> {
        let response = self
            .client
            .post(&self.endpoints.token)
            .header(reqwest::header::CONTENT_TYPE, FORM_CONTENT_TYPE)
            .body(form_body(form))
            .send()
            .await
            .map_err(|_| GoogleError::Unreachable)?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|_| GoogleError::Unreachable)?;
        if !status.is_success() {
            return Err(
                if body.get("error").and_then(Value::as_str) == Some("invalid_grant") {
                    GoogleError::SignInExpired
                } else {
                    GoogleError::ExchangeFailed
                },
            );
        }
        serde_json::from_value(body).map_err(|_| GoogleError::Unreachable)
    }

    async fn profile_email(&self, access: &str) -> Result<String, GoogleError> {
        let response = self
            .client
            .get(format!("{}/users/me/profile", self.endpoints.gmail))
            .bearer_auth(access)
            .send()
            .await
            .map_err(|_| GoogleError::Unreachable)?;
        if !response.status().is_success() {
            return Err(GoogleError::ExchangeFailed);
        }
        let body: Value = response
            .json()
            .await
            .map_err(|_| GoogleError::Unreachable)?;
        body.get("emailAddress")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(GoogleError::Unreachable)
    }

    /// A valid access token, refreshing it when it is about to expire.
    ///
    /// # Errors
    ///
    /// [`GoogleError::NotConnected`] with nobody signed in; [`GoogleError::SignInExpired`] when Google no longer honours the stored
    /// sign-in (it is removed, so the next step is signing in again); other variants for transport and configuration faults.
    pub async fn access_token(&self) -> Result<String, GoogleError> {
        let mut cache = self.access.lock().await;
        if let Some(cached) = cache.as_ref()
            && cached.expires > Instant::now() + REFRESH_MARGIN
        {
            return Ok(cached.token.clone());
        }
        let stored = self.read_stored().ok_or(GoogleError::NotConnected)?;
        let (client_id, secret) = self.credentials()?;
        let mut form = vec![
            ("grant_type", "refresh_token".to_owned()),
            ("refresh_token", stored.refresh_token.clone()),
            ("client_id", client_id),
        ];
        if let Some(secret) = secret {
            form.push(("client_secret", secret));
        }
        match self.token_request(&form).await {
            Ok(reply) => {
                let token = reply.access_token.ok_or(GoogleError::ExchangeFailed)?;
                *cache = Some(Cached {
                    token: token.clone(),
                    expires: Instant::now() + Duration::from_secs(reply.expires_in.unwrap_or(3000)),
                });
                Ok(token)
            }
            Err(GoogleError::SignInExpired) => {
                // Revoked or expired at Google: the stored grant is useless, so it is removed rather than retried forever.
                *cache = None;
                let _ = std::fs::remove_file(self.token_path());
                Err(GoogleError::SignInExpired)
            }
            Err(other) => Err(other),
        }
    }

    async fn revoke(&self, token: &str) {
        let _ = self
            .client
            .post(&self.endpoints.revoke)
            .header(reqwest::header::CONTENT_TYPE, FORM_CONTENT_TYPE)
            .body(form_body(&[("token", token.to_owned())]))
            .send()
            .await;
    }

    /// Signs out: the grant is revoked at Google (best effort) and the stored sign-in is deleted either way.
    ///
    /// # Errors
    ///
    /// [`GoogleError::Storage`] when the stored file cannot be removed.
    pub async fn disconnect(&self) -> Result<(), GoogleError> {
        if let Some(stored) = self.read_stored() {
            self.revoke(&stored.refresh_token).await;
        }
        *self.access.lock().await = None;
        self.memory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pending = None;
        match std::fs::remove_file(self.token_path()) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(GoogleError::Storage),
        }
    }
}

#[cfg(test)]
#[path = "google_account_tests.rs"]
pub(crate) mod tests;
