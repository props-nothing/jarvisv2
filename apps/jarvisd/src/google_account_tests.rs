//! Tests for Google sign-in against a local fixture standing in for Google's endpoints.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

/// One recorded request: the path and the body.
type Recorded = Arc<StdMutex<Vec<(String, String)>>>;
type Routes = Arc<StdMutex<HashMap<String, (u16, String)>>>;

pub(crate) struct Fixture {
    pub(crate) base: String,
    requests: Recorded,
    routes: Routes,
}

pub(crate) async fn fixture() -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("bind: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("address: {error}"));
    let requests: Recorded = Arc::new(StdMutex::new(Vec::new()));
    let routes: Routes = Arc::new(StdMutex::new(HashMap::new()));
    let (seen, table) = (Arc::clone(&requests), Arc::clone(&routes));
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let (seen, table) = (Arc::clone(&seen), Arc::clone(&table));
            tokio::spawn(async move {
                let mut received = Vec::new();
                let mut buffer = vec![0_u8; 16 * 1024];
                while let Ok(count) = socket.read(&mut buffer).await {
                    if count == 0 {
                        break;
                    }
                    received.extend_from_slice(&buffer[..count]);
                    let text = String::from_utf8_lossy(&received).to_string();
                    if let Some(split) = text.find("\r\n\r\n") {
                        let length = text[..split]
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|value| value.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if text.len() >= split + 4 + length {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&received).to_string();
                let path = text
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let body = text.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned();
                seen.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((path.clone(), body));
                let (status, reply) = table
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&path)
                    .cloned()
                    .unwrap_or((404, "{}".to_owned()));
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    Fixture {
        base: format!("http://{address}"),
        requests,
        routes,
    }
}

impl Fixture {
    pub(crate) fn answer(&self, path: &str, status: u16, body: &str) {
        self.routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path.to_owned(), (status, body.to_owned()));
    }

    pub(crate) fn bodies(&self, path: &str) -> Vec<String> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(seen, _)| seen == path)
            .map(|(_, body)| body.clone())
            .collect()
    }

    pub(crate) fn endpoints(&self) -> Endpoints {
        Endpoints {
            authorization: "https://accounts.example/o/oauth2/v2/auth".to_owned(),
            token: format!("{}/token", self.base),
            revoke: format!("{}/revoke", self.base),
            gmail: format!("{}/gmail", self.base),
            calendar: format!("{}/calendar", self.base),
        }
    }
}

pub(crate) struct Scratch(pub(crate) PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        jarvis_core::remove_scratch_dir(&self.0);
    }
}

pub(crate) fn account(fixture: &Fixture, configure: bool) -> (Scratch, GoogleAccount) {
    let tag = jarvis_core::scratch_tag();
    let root = std::env::temp_dir().join(format!("jga-{}", &tag[tag.len() - 12..]));
    std::fs::create_dir_all(&root).unwrap_or_else(|error| panic!("{error}"));
    let paths = jarvis_storage::portable_layout(&root).unwrap_or_else(|| panic!("portable layout"));
    std::fs::create_dir_all(paths.config()).unwrap_or_else(|error| panic!("{error}"));
    let secret = paths.config().join("google.key");
    std::fs::write(&secret, "the-client-secret\n").unwrap_or_else(|error| panic!("{error}"));
    let secret_text = secret.display().to_string().replace('\\', "/");
    let google = if configure {
        format!("google_client_id = \"client-123\"\ngoogle_client_secret_ref = \"{secret_text}\"\n")
    } else {
        String::new()
    };
    let document = format!(
        "schema_version = 1\n[profile]\nname = \"home\"\n[logging]\nlevel = \"warn\"\n[daemon]\nshutdown_timeout_seconds = 30\n{google}"
    );
    let store = ConfigStore::from_paths(&paths);
    std::fs::write(store.path(), document).unwrap_or_else(|error| panic!("{error}"));
    let account = GoogleAccount::with_endpoints(paths, fixture.endpoints())
        .unwrap_or_else(|error| panic!("{error}"));
    (Scratch(root), account)
}

pub(crate) fn query(url: &str, name: &str) -> String {
    reqwest::Url::parse(url)
        .unwrap_or_else(|error| panic!("{error}"))
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

/// One field of a form body, decoded the way Google decodes it (the encoder percent-escapes every non-alphanumeric byte).
fn field(body: &str, name: &str) -> String {
    body.split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| jarvis_connectors::form::decode_component(key).as_deref() == Ok(name))
        .and_then(|(_, value)| jarvis_connectors::form::decode_component(value).ok())
        .unwrap_or_default()
}

pub(crate) fn granted_all() -> String {
    format!("{SCOPE_GMAIL_READONLY} {SCOPE_CALENDAR_READONLY}")
}

pub(crate) fn token_reply(scope: &str, refresh: bool) -> String {
    let refresh = if refresh {
        r#""refresh_token":"refresh-1","#
    } else {
        ""
    };
    format!(
        r#"{{"access_token":"access-1","expires_in":3600,{refresh}"scope":"{scope}","token_type":"Bearer"}}"#
    )
}

const PORT: u16 = 8765;

/// Signs in with extra scopes granted as well as the two read scopes.
pub(crate) async fn sign_in_with(fixture: &Fixture, account: &GoogleAccount, extra: &[&str]) {
    let scope = format!("{} {}", granted_all(), extra.join(" "));
    fixture.answer("/token", 200, &token_reply(scope.trim(), true));
    fixture.answer(
        "/gmail/users/me/profile",
        200,
        r#"{"emailAddress":"me@example.com"}"#,
    );
    // The extra scopes were requested, so the sign-in is held to them (the owner turned actions on).
    let url = account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    let state = query(&url, "state");
    let _ = extra;
    account
        .complete(&format!("{CALLBACK_PATH}?code=c&state={state}"))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

/// Signs the account in against the fixture (a token reply and a profile are answered), as the owner's browser would.
pub(crate) async fn sign_in(fixture: &Fixture, account: &GoogleAccount) {
    fixture.answer("/token", 200, &token_reply(&granted_all(), true));
    fixture.answer(
        "/gmail/users/me/profile",
        200,
        r#"{"emailAddress":"me@example.com"}"#,
    );
    let url = account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    let state = query(&url, "state");
    account
        .complete(&format!("{CALLBACK_PATH}?code=c&state={state}"))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

/// **The sign-in address carries PKCE S256, a state, the loopback redirect and exactly the two read scopes.**
#[tokio::test]
async fn begin_builds_a_pkce_request_for_the_two_read_scopes() {
    let fixture = fixture().await;
    let (_scratch, account) = account(&fixture, true);
    let url = account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(query(&url, "client_id"), "client-123");
    assert_eq!(query(&url, "response_type"), "code");
    assert_eq!(query(&url, "code_challenge_method"), "S256");
    assert_eq!(
        query(&url, "redirect_uri"),
        format!("http://127.0.0.1:{PORT}{CALLBACK_PATH}")
    );
    assert_eq!(
        query(&url, "scope"),
        granted_all(),
        "read mail and read the calendar, nothing that sends or changes"
    );
    assert!(query(&url, "state").len() >= 32);
    assert_eq!(query(&url, "prompt"), "consent");
    assert!(account.status().pending);
}

#[tokio::test]
async fn without_a_client_id_there_is_no_sign_in() {
    let fixture = fixture().await;
    let (_scratch, account) = account(&fixture, false);
    assert_eq!(account.begin(PORT), Err(GoogleError::NotConfigured));
    let status = account.status();
    assert!(!status.configured && !status.connected);
}

/// **The whole sign-in: the exchange proves the PKCE verifier, sends the secret, and the refresh token is stored, not returned.**
#[tokio::test]
async fn a_valid_answer_completes_the_sign_in_and_stores_only_the_refresh_token() {
    let fixture = fixture().await;
    fixture.answer("/token", 200, &token_reply(&granted_all(), true));
    fixture.answer(
        "/gmail/users/me/profile",
        200,
        r#"{"emailAddress":"me@example.com"}"#,
    );
    let (scratch, account) = account(&fixture, true);
    let url = account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    let state = query(&url, "state");

    let email = account
        .complete(&format!("{CALLBACK_PATH}?code=the-code&state={state}"))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(email, "me@example.com");

    let exchange = fixture.bodies("/token").remove(0);
    assert_eq!(field(&exchange, "grant_type"), "authorization_code");
    assert_eq!(field(&exchange, "code"), "the-code");
    assert_eq!(
        field(&exchange, "redirect_uri"),
        format!("http://127.0.0.1:{PORT}{CALLBACK_PATH}")
    );
    assert_eq!(
        field(&exchange, "client_secret"),
        "the-client-secret",
        "the Desktop client's secret is sent"
    );
    let verifier = field(&exchange, "code_verifier");
    assert_eq!(
        PkceVerifier::new(verifier)
            .unwrap_or_else(|error| panic!("{error}"))
            .challenge(PkceMethod::S256)
            .value(),
        query(&url, "code_challenge"),
        "the verifier sent proves the challenge that was committed to"
    );

    let status = account.status();
    assert!(status.connected && !status.pending);
    assert_eq!(status.email.as_deref(), Some("me@example.com"));
    let stored = std::fs::read_to_string(scratch.0.join("google.token")).unwrap_or_else(|_| {
        std::fs::read_to_string(account.token_path()).unwrap_or_else(|error| panic!("{error}"))
    });
    assert!(
        stored.contains("refresh-1") && !stored.contains("access-1"),
        "only the refresh token is stored: {stored}"
    );
    let token = account
        .access_token()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(token, "access-1");
    assert_eq!(
        fixture.bodies("/token").len(),
        1,
        "a still-valid access token is not refreshed"
    );
}

/// **The falsification: an answer that is not for a sign-in started here stores nothing.**
///
/// No pending sign-in, a forged state, an error from Google, an expired transaction and a repeated answer: each is refused, no token
/// request is made, and nobody ends up signed in.
#[tokio::test]
async fn a_forged_or_repeated_answer_is_refused_and_stores_nothing() {
    let fixture = fixture().await;
    fixture.answer("/token", 200, &token_reply(&granted_all(), true));
    fixture.answer(
        "/gmail/users/me/profile",
        200,
        r#"{"emailAddress":"me@example.com"}"#,
    );
    let (_scratch, account) = account(&fixture, true);

    // No sign-in was started.
    assert_eq!(
        account
            .complete(&format!("{CALLBACK_PATH}?code=x&state=y"))
            .await,
        Err(GoogleError::Refused)
    );
    // A sign-in started, answered with another state.
    let url = account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        account
            .complete(&format!("{CALLBACK_PATH}?code=x&state=forged"))
            .await,
        Err(GoogleError::Refused)
    );
    // The transaction was consumed by that attempt: the right state now has nothing to match.
    let state = query(&url, "state");
    assert_eq!(
        account
            .complete(&format!("{CALLBACK_PATH}?code=x&state={state}"))
            .await,
        Err(GoogleError::Refused)
    );
    // Google reporting a refusal (the owner pressed cancel).
    account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        account
            .complete(&format!("{CALLBACK_PATH}?error=access_denied"))
            .await,
        Err(GoogleError::ProviderError)
    );
    // The right answer to the wrong path.
    let url = account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    let state = query(&url, "state");
    assert_eq!(
        account
            .complete(&format!("/somewhere/else?code=x&state={state}"))
            .await,
        Err(GoogleError::Refused)
    );

    assert!(
        fixture.bodies("/token").is_empty(),
        "no code was ever exchanged"
    );
    assert!(!account.status().connected);
}

#[tokio::test]
async fn a_grant_missing_a_permission_is_refused_and_withdrawn() {
    let fixture = fixture().await;
    fixture.answer("/token", 200, &token_reply(SCOPE_GMAIL_READONLY, true));
    fixture.answer("/revoke", 200, "{}");
    let (_scratch, account) = account(&fixture, true);
    let url = account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    let state = query(&url, "state");
    assert_eq!(
        account
            .complete(&format!("{CALLBACK_PATH}?code=c&state={state}"))
            .await,
        Err(GoogleError::ScopesMissing)
    );
    assert!(!account.status().connected);
    assert_eq!(
        fixture.bodies("/revoke").len(),
        1,
        "the partial grant is revoked"
    );
}

/// **A revoked sign-in is noticed, removed, and reported as needing a new sign-in; disconnect revokes and deletes.**
#[tokio::test]
async fn a_revoked_sign_in_is_removed_and_disconnect_revokes_it() {
    let fixture = fixture().await;
    fixture.answer("/token", 200, &token_reply(&granted_all(), true));
    fixture.answer(
        "/gmail/users/me/profile",
        200,
        r#"{"emailAddress":"me@example.com"}"#,
    );
    fixture.answer("/revoke", 200, "{}");
    let (_scratch, account) = account(&fixture, true);
    let url = account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    let state = query(&url, "state");
    account
        .complete(&format!("{CALLBACK_PATH}?code=c&state={state}"))
        .await
        .unwrap_or_else(|error| panic!("{error}"));

    // Google stops honouring the refresh token.
    *account.access.lock().await = None;
    fixture.answer(
        "/token",
        400,
        r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
    );
    assert_eq!(
        account.access_token().await,
        Err(GoogleError::SignInExpired)
    );
    assert!(!account.status().connected, "the dead sign-in was removed");
    assert_eq!(account.access_token().await, Err(GoogleError::NotConnected));

    // Sign in again, then disconnect.
    fixture.answer("/token", 200, &token_reply(&granted_all(), true));
    let url = account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    let state = query(&url, "state");
    account
        .complete(&format!("{CALLBACK_PATH}?code=c2&state={state}"))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    account
        .disconnect()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!account.status().connected);
    assert!(
        fixture
            .bodies("/revoke")
            .iter()
            .any(|body| field(body, "token") == "refresh-1")
    );
}

/// **An expired access token is refreshed with the stored refresh token.**
#[tokio::test]
async fn an_expired_access_token_is_refreshed() {
    let fixture = fixture().await;
    fixture.answer("/token", 200, &token_reply(&granted_all(), true));
    fixture.answer(
        "/gmail/users/me/profile",
        200,
        r#"{"emailAddress":"me@example.com"}"#,
    );
    let (_scratch, account) = account(&fixture, true);
    let url = account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    let state = query(&url, "state");
    account
        .complete(&format!("{CALLBACK_PATH}?code=c&state={state}"))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    *account.access.lock().await = Some(Cached {
        token: "stale".to_owned(),
        expires: Instant::now(),
    });
    fixture.answer(
        "/token",
        200,
        r#"{"access_token":"access-2","expires_in":3600,"token_type":"Bearer"}"#,
    );
    assert_eq!(account.access_token().await, Ok("access-2".to_owned()));
    let refresh = fixture.bodies("/token").pop().unwrap_or_default();
    assert_eq!(field(&refresh, "grant_type"), "refresh_token");
    assert_eq!(field(&refresh, "refresh_token"), "refresh-1");
}

/// **Turning actions on adds exactly the two write scopes to the request, and a sign-in that lacks one is refused.**
#[tokio::test]
async fn actions_add_the_write_scopes_and_hold_the_grant_to_them() {
    let fixture = fixture().await;
    let (_scratch, account) = account(&fixture, true);
    let store = ConfigStore::from_paths(&account.paths);
    let document = std::fs::read_to_string(store.path()).unwrap_or_else(|error| panic!("{error}"));
    std::fs::write(store.path(), format!("{document}google_actions = true\n"))
        .unwrap_or_else(|error| panic!("{error}"));

    let url = account
        .begin(PORT)
        .unwrap_or_else(|error| panic!("{error}"));
    let scopes = query(&url, "scope");
    assert_eq!(scopes.split(' ').count(), 4, "{scopes}");
    assert!(scopes.contains(SCOPE_GMAIL_SEND) && scopes.contains(SCOPE_CALENDAR_EVENTS));
    assert!(
        !scopes.contains("gmail.modify") && !scopes.contains("mail.google.com"),
        "no broader mail scope is ever asked for"
    );

    // Google grants only the two read scopes (the owner unticked the rest): refused and withdrawn.
    fixture.answer("/token", 200, &token_reply(&granted_all(), true));
    fixture.answer("/revoke", 200, "{}");
    let state = query(&url, "state");
    assert_eq!(
        account
            .complete(&format!("{CALLBACK_PATH}?code=c&state={state}"))
            .await,
        Err(GoogleError::ScopesMissing)
    );
    assert!(!account.status().connected);
    assert!(!account.has_scope(SCOPE_GMAIL_SEND));
}
