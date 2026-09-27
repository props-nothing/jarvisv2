//! Tests for the token exchange: the seam that finally produces [`TokenRequestOutcome`].
//!
//! The load-bearing assertion is the **retry-safety distinction**. `TokenRequestOutcome` separates `NeverSent`
//! from `SentAnswerUnknown` because RFC 9700 §4.2.4 makes a retry of a lost-answer request able to consume the
//! authorization code and destroy a working grant. That type was written several rounds ago with a doc calling
//! the distinction consequential, and until this file existed **nothing produced either variant** — every
//! construction site was a test. So the cases below are the two directions, asserted with the consequence
//! rather than the variant alone.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::google::credential::AccessToken;
use crate::google::transport::{GoogleTransport, HttpMethod, TransportFailure, TransportResponse};
use std::sync::Mutex;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

/// The identity every exchange test uses.
fn identity() -> ExchangeIdentity {
    must(
        ExchangeIdentity::new(
            "123456789.apps.googleusercontent.invalid",
            "http://127.0.0.1/",
        ),
        "a valid identity",
    )
}

/// The PKCE verifier from `token_tests`, rebuilt here because the two modules are separate.
fn verifier() -> Secret {
    must(
        Secret::new(
            "code_verifier",
            "abcdefghijklmnopqrstuvwxyz-._~0123456789ABCDEFG",
            MAX_CODE_VERIFIER_CHARS,
        ),
        "a valid verifier",
    )
}

/// A transport that answers a form `POST` from a script, so the exchange is driven without a socket.
///
/// It **records** the request it was given, because the claim under test is about what the exchange sends —
/// the endpoint, the content type, and the body — and an exchange asserted only on its return value would
/// satisfy the same assertions while sending nothing.
struct ScriptedForm {
    result: Result<TransportResponse, TransportFailure>,
    seen: Mutex<Vec<(String, String, String)>>,
}

impl ScriptedForm {
    fn answering(result: Result<TransportResponse, TransportFailure>) -> Self {
        Self {
            result,
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Returns the requests received, as `(url, content_type, body)`.
    fn requests(&self) -> Vec<(String, String, String)> {
        self.seen
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl GoogleTransport for ScriptedForm {
    async fn send(
        &self,
        _method: HttpMethod,
        _request: &crate::google::request::HttpRequest,
        _token: &AccessToken,
    ) -> Result<TransportResponse, TransportFailure> {
        // A read reaching this transport is a mistake in the test, not a provider condition: the exchange must
        // only ever use `send_form`, and answering here would make a wrong call look like a right one.
        panic!("the token exchange must not use the read path");
    }

    async fn send_form(
        &self,
        request: &crate::google::request::FormRequest,
    ) -> Result<TransportResponse, TransportFailure> {
        if let Ok(mut guard) = self.seen.lock() {
            guard.push((
                request.url().to_owned(),
                request.content_type().to_owned(),
                request.rendered_body().to_owned(),
            ));
        }
        self.result.clone()
    }
}

/// A grant whose scope echoes what the caller asked for.
fn granted_body() -> &'static str {
    r#"{"access_token":"ya29.secret","token_type":"Bearer","expires_in":3599,"scope":"https://www.googleapis.com/auth/gmail.readonly"}"#
}

/// A refresh reference, which is metadata rather than a secret.
fn reference() -> jarvis_core::SecretRef {
    must(
        jarvis_core::SecretRef::new("keychain", "google-refresh-1", "oauth-refresh"),
        "a usable reference",
    )
}

/// The scopes a caller previously held, for the token set's comparison.
fn previous() -> Vec<String> {
    vec!["https://www.googleapis.com/auth/gmail.readonly".to_owned()]
}

/// The scopes this request asked for.
fn requested() -> Vec<String> {
    vec!["https://www.googleapis.com/auth/gmail.readonly".to_owned()]
}

/// A form request the way a caller builds one: endpoint from the identity, body from the parameters.
fn request() -> crate::google::request::FormRequest {
    let identity = identity();
    let code = must(
        Secret::new("code", "4/0Axyz", MAX_AUTHORIZATION_CODE_CHARS),
        "a valid code",
    );
    let parameters = must(
        exchange_code(&code, &verifier(), &identity),
        "the exchange must build",
    );
    crate::google::request::FormRequest::new(identity.endpoint(), body(&parameters), content_type())
}

#[tokio::test]
async fn a_granted_answer_becomes_an_answered_outcome() {
    // The positive control: without it, the failure cases below would pass on an exchange that never worked.
    let transport = ScriptedForm::answering(Ok(TransportResponse {
        status: 200,
        retry_after: None,
        body: granted_body().to_owned(),
    }));
    let outcome = must(
        exchange(
            &transport,
            &request(),
            Some(reference()),
            &previous(),
            &requested(),
        )
        .await,
        "a 200 with a grant must be an outcome, not an error",
    );
    let set = outcome
        .token_set()
        .unwrap_or_else(|| panic!("a granted answer must carry a token set"));
    assert_eq!(set.lifetime_seconds, Some(3599));
    assert_eq!(set.granted_scopes, previous());
    assert_eq!(set.refresh_reference.as_ref(), Some(&reference()));
    assert!(
        !outcome.permits_automatic_retry(),
        "an answered request is not retried: the code is already consumed"
    );

    // And the transport really received the request, on the endpoint the identity names with the media type the
    // protocol requires — so the assertion above is about an exchange rather than about a value this process
    // made up.
    let seen = transport.requests();
    assert_eq!(seen.len(), 1, "exactly one request");
    assert_eq!(seen[0].0, "https://oauth2.googleapis.com/token");
    assert_eq!(seen[0].1, "application/x-www-form-urlencoded");
    // The body is the raw values escaped **once** (`ADR-0072`): a `%25` would mean something was encoded at rest.
    assert!(
        !seen[0].2.contains("%25"),
        "the body must not be double-encoded: {}",
        seen[0].2
    );
    assert!(seen[0].2.contains("grant%5Ftype=authorization%5Fcode"));
}

#[tokio::test]
async fn a_refusal_is_an_outcome_and_not_an_error() {
    // RFC 6749 §5.2 makes a refusal a well-formed body, so it is `Refused` rather than an `Err`. Returning an
    // error would lose the machine-readable code, which is everything the caller's next decision needs.
    let transport = ScriptedForm::answering(Ok(TransportResponse {
        status: 400,
        retry_after: None,
        body: r#"{"error":"invalid_grant"}"#.to_owned(),
    }));
    let outcome = must(
        exchange(
            &transport,
            &request(),
            Some(reference()),
            &previous(),
            &requested(),
        )
        .await,
        "a refusal must be reported rather than refused",
    );
    let failure = match &outcome {
        TokenRequestOutcome::Refused(failure) => failure,
        other => panic!("a refusal must be `Refused`, got {other:?}"),
    };
    assert_eq!(failure.error, "invalid_grant");
    assert!(outcome.token_set().is_none());
    assert!(!outcome.permits_automatic_retry());
}

#[tokio::test]
async fn the_two_transport_directions_are_never_sent_and_sent_answer_unknown() {
    // **The assertion `TokenRequestOutcome` exists for.** The two are indistinguishable from the request side
    // and opposite operationally, so each transport failure must land on the correct one — and the pairing is
    // asserted against `may_have_reached_the_provider` rather than against a hand-written expectation, so a new
    // variant cannot be classified by omission.
    let certain = [
        TransportFailure::Connect,
        TransportFailure::Refused {
            reason: "the transport refused before writing",
        },
    ];
    for failure in certain {
        assert!(
            !failure.may_have_reached_the_provider(),
            "{failure:?} is certain, so the fixture is wrong"
        );
        let transport = ScriptedForm::answering(Err(failure));
        let outcome = must(
            exchange(&transport, &request(), None, &previous(), &requested()).await,
            "a transport failure is an outcome, never an error",
        );
        match outcome {
            TokenRequestOutcome::NeverSent { reason } => {
                assert!(!reason.is_empty(), "{failure:?} must state a reason");
            }
            other => panic!("{failure:?} must be `NeverSent`, got {other:?}"),
        }
        assert!(
            outcome.permits_automatic_retry(),
            "{failure:?}: nothing reached the provider, so a retry is free"
        );
    }

    let ambiguous = [
        TransportFailure::Send,
        TransportFailure::Timeout,
        TransportFailure::Body,
    ];
    for failure in ambiguous {
        assert!(
            failure.may_have_reached_the_provider(),
            "{failure:?} is ambiguous, so the fixture is wrong"
        );
        let transport = ScriptedForm::answering(Err(failure));
        let outcome = must(
            exchange(&transport, &request(), None, &previous(), &requested()).await,
            "a transport failure is an outcome, never an error",
        );
        assert_eq!(
            outcome,
            TokenRequestOutcome::SentAnswerUnknown,
            "{failure:?} may have been written, so a retry could repeat an effect"
        );
        assert!(
            !outcome.permits_automatic_retry(),
            "{failure:?}: a retry may consume the code and destroy the grant the first attempt issued"
        );
    }
}

#[tokio::test]
async fn an_answer_that_cannot_be_read_is_an_error_and_not_a_refusal() {
    // **The distinction that keeps a reader out of the wrong place.** A body that is neither a grant nor a
    // refusal is an outcome that could not be established — a different thing from the provider answering "no".
    // Folding it into `Refused` would send a user to a consent screen when the fault is a body this client
    // cannot parse.
    let transport = ScriptedForm::answering(Ok(TransportResponse {
        status: 400,
        retry_after: None,
        body: "<html>a proxy's page</html>".to_owned(),
    }));
    let error = match exchange(&transport, &request(), None, &previous(), &requested()).await {
        Ok(outcome) => panic!("an unreadable body must not be an outcome, got {outcome:?}"),
        Err(error) => error,
    };
    assert!(
        matches!(error, TokenRequestError::Body { .. }),
        "got {error:?}"
    );

    // And a grant this platform cannot use is `UnusableGrant` rather than `Body`: the server *did* answer, and
    // it stated a token type this client will not accept.
    let transport = ScriptedForm::answering(Ok(TransportResponse {
        status: 200,
        retry_after: None,
        body: r#"{"access_token":"ya29.secret","token_type":"mac"}"#.to_owned(),
    }));
    let error = match exchange(&transport, &request(), None, &previous(), &requested()).await {
        Ok(outcome) => panic!("an unusable grant must not be an outcome, got {outcome:?}"),
        Err(error) => error,
    };
    assert!(
        matches!(error, TokenRequestError::UnusableGrant { .. }),
        "got {error:?}"
    );
}

#[test]
fn a_form_request_cannot_render_its_credential_bearing_body() {
    // `FormRequest` holds the `code` and the `refresh_token` in its rendered body, so its `Debug` is
    // hand-written for the reason `ADR-0061` records: a derived one would print whatever the struct holds, and
    // one `{:?}` in a log or a test failure message would render the credential.
    let request = crate::google::request::FormRequest::new(
        "https://oauth2.googleapis.com/token",
        "code=4%2F0Axyz&grant%5Ftype=authorization%5Fcode",
        "application/x-www-form-urlencoded",
    );
    let rendered = format!("{request:?}");
    assert!(
        !rendered.contains("0Axyz"),
        "the body must not be rendered: {rendered}"
    );
    assert!(rendered.contains("[REDACTED]"), "{rendered}");
    assert!(
        rendered.contains("POST"),
        "the shape is what a diagnostic needs: {rendered}"
    );
    // The **length** is reported, because it is not the value and it is what distinguishes two requests.
    assert!(rendered.contains("chars:"), "{rendered}");
    // The accessor is named so the exposure is visible at every call site.
    assert!(request.rendered_body().contains("0Axyz"));
}

/// A refresh request the way a caller builds one.
fn refresh_request() -> crate::google::request::FormRequest {
    let identity = identity();
    let token = must(
        Secret::new(
            REFRESH_TOKEN_PARAMETER,
            "1//0gLongLivedRefresh",
            MAX_REFRESH_TOKEN_CHARS,
        ),
        "a valid refresh token",
    );
    crate::google::request::FormRequest::new(
        identity.endpoint(),
        body(&refresh(&token, &identity)),
        content_type(),
    )
}

#[tokio::test]
async fn a_refresh_that_did_not_rotate_is_refreshed_and_keeps_the_reference() {
    // **The producer `RefreshExchange` never had.** `classify_refresh` and `refresh` had no caller outside
    // their tests, so the RFC 9700 §4.14.2 replay detection they describe was documentation rather than a
    // capability — the same defect `ADR-0073` records for `TokenRequestOutcome`, one layer over.
    let transport = ScriptedForm::answering(Ok(TransportResponse {
        status: 200,
        retry_after: None,
        // Google's documented refresh does NOT return a new refresh token, so this is the expected shape for
        // this provider rather than a degradation.
        body: granted_body().to_owned(),
    }));
    let exchange = must(
        refresh_with(
            &transport,
            &refresh_request(),
            Some(reference()),
            &previous(),
            &requested(),
            false,
        )
        .await,
        "a refresh must be classified, not dropped",
    );
    // `Refreshed` **detects** that no rotation happened; a caller that needs rotation for replay detection
    // learns from this that the provider does not rotate (the research record's Unresolved Question 3).
    assert_eq!(exchange.outcome, RefreshOutcome::Refreshed);
    assert!(
        exchange.rotated_reference.is_none(),
        "the previous reference still stands when no rotation happened"
    );
    let set = exchange
        .token_set
        .as_ref()
        .unwrap_or_else(|| panic!("a successful refresh carries a token set"));
    assert_eq!(set.lifetime_seconds, Some(3599));
    assert!(!exchange.outcome.needs_user());

    // And the request was a real one: the same endpoint and media type, with the refresh token in the body.
    let seen = transport.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "https://oauth2.googleapis.com/token");
    assert!(
        seen[0]
            .2
            .contains("refresh%5Ftoken=1%2F%2F0gLongLivedRefresh"),
        "the stored token must be presented, escaped once: {}",
        seen[0].2
    );
}

#[tokio::test]
async fn a_rotation_reports_the_new_reference_and_a_lost_one_is_visible_the_same_way() {
    // `Rotated` is decided by whether new material **arrived**, never by whether the caller stored it: a caller
    // that failed to store one has a defect of its own, and reporting `Refreshed` would hide the half of the
    // exchange that makes replay detectable.
    let rotating = r#"{"access_token":"ya29.secret","token_type":"Bearer","expires_in":3599,"refresh_token":"1//0gNew","scope":"https://www.googleapis.com/auth/gmail.readonly"}"#;
    let transport = ScriptedForm::answering(Ok(TransportResponse {
        status: 200,
        retry_after: None,
        body: rotating.to_owned(),
    }));
    let stored = must(
        refresh_with(
            &transport,
            &refresh_request(),
            Some(reference()),
            &previous(),
            &requested(),
            false,
        )
        .await,
        "a rotation must be classified",
    );
    assert_eq!(stored.outcome, RefreshOutcome::Rotated);
    assert_eq!(
        stored.rotated_reference.as_ref(),
        Some(&reference()),
        "a rotation names where the new material went, so a caller can retire the old reference"
    );

    // The same rotation with **no** reference supplied is still `Rotated`. This is the case that makes the rule
    // falsifiable: a classifier reading the caller's storage would call it `Refreshed`.
    let transport = ScriptedForm::answering(Ok(TransportResponse {
        status: 200,
        retry_after: None,
        body: rotating.to_owned(),
    }));
    let unrecorded = must(
        refresh_with(
            &transport,
            &refresh_request(),
            None,
            &previous(),
            &requested(),
            false,
        )
        .await,
        "a rotation must be classified",
    );
    assert_eq!(
        unrecorded.outcome,
        RefreshOutcome::Rotated,
        "arrival decides rotation, not storage"
    );
    assert!(unrecorded.rotated_reference.is_none());
}

#[tokio::test]
async fn a_refresh_transport_failure_is_transient_and_the_second_attempt_asks_the_user() {
    // **The asymmetry against the code exchange, and why it is safe.** None of `RefreshOutcome`'s five variants
    // means "the request may have been written", and the reason this is tolerable is the consequence: a refresh
    // presents the *stored* token, so if a rotation silently landed the stored reference is already invalid and
    // the next attempt fails with `invalid_grant` → `Expired`/`Revoked` → `needs_user`. So the ambiguous case
    // self-corrects over one extra call, where retrying a lost *code* exchange could revoke tokens.
    let transport = ScriptedForm::answering(Err(TransportFailure::Timeout));
    let exchange = must(
        refresh_with(
            &transport,
            &refresh_request(),
            Some(reference()),
            &previous(),
            &requested(),
            false,
        )
        .await,
        "a transport failure is an outcome, never an error",
    );
    assert_eq!(exchange.outcome, RefreshOutcome::Transient);
    assert!(
        exchange.outcome.is_safe_to_retry(),
        "the request may have been written, but a refresh is safe to attempt again"
    );
    assert!(
        !exchange.outcome.needs_user(),
        "a transient failure must not send the user to a consent screen"
    );
    assert!(exchange.token_set.is_none());
    assert!(exchange.rotated_reference.is_none());

    // The self-correction, driven: the second attempt gets `invalid_grant`, which is `Expired` — so the caller
    // that retried once now knows a user is needed without having had to distinguish the two cases.
    let second = ScriptedForm::answering(Ok(TransportResponse {
        status: 400,
        retry_after: None,
        body: r#"{"error":"invalid_grant"}"#.to_owned(),
    }));
    let after = must(
        refresh_with(
            &second,
            &refresh_request(),
            Some(reference()),
            &previous(),
            &requested(),
            false,
        )
        .await,
        "the second attempt must be classified",
    );
    assert_eq!(after.outcome, RefreshOutcome::Expired);
    assert!(after.outcome.needs_user());
}

#[tokio::test]
async fn a_refresh_refusal_is_ordered_so_an_outage_outranks_the_error_code() {
    // The ordering `classify` records, asserted through the producer rather than the classifier: a provider
    // behind a proxy can answer a `503` carrying `invalid_grant`, and reading the code first would send a user
    // to a consent screen **during an outage**. The control is the same code without the outage, which IS the
    // user's problem.
    let outage = ScriptedForm::answering(Ok(TransportResponse {
        status: 503,
        retry_after: Some(crate::google::transport::RetryAfter::Seconds(30)),
        body: r#"{"error":"invalid_grant"}"#.to_owned(),
    }));
    let during = must(
        refresh_with(
            &outage,
            &refresh_request(),
            None,
            &previous(),
            &requested(),
            false,
        )
        .await,
        "a 503 carrying a grant code must be classified",
    );
    assert_eq!(
        during.outcome,
        RefreshOutcome::Transient,
        "the outage is the more specific observation"
    );

    let user = ScriptedForm::answering(Ok(TransportResponse {
        status: 400,
        retry_after: None,
        body: r#"{"error":"invalid_grant"}"#.to_owned(),
    }));
    let after = must(
        refresh_with(
            &user,
            &refresh_request(),
            None,
            &previous(),
            &requested(),
            false,
        )
        .await,
        "the same code without the outage must be classified",
    );
    assert_eq!(after.outcome, RefreshOutcome::Expired);
    assert!(
        after.outcome.needs_user(),
        "without an outage the code IS the user's problem"
    );
}

#[test]
fn a_failure_reason_agrees_with_whether_it_may_have_been_written() {
    // `TransportFailure::reason` is reported as "never sent", so a variant that may have reached the provider
    // must not answer with a certain sentence. The pairing is checked against `may_have_reached_the_provider`
    // rather than against the text, so the assertion is about the classification rather than the wording.
    for failure in [
        TransportFailure::Connect,
        TransportFailure::Send,
        TransportFailure::Timeout,
        TransportFailure::Body,
        TransportFailure::Refused { reason: "no" },
    ] {
        let reason = failure.reason();
        assert!(!reason.is_empty(), "{failure:?}");
        if failure.may_have_reached_the_provider() {
            // A reassuring sentence here is the defect: the caller reports this as `NeverSent` only when the
            // predicate is false, and the text must not contradict the predicate for the other callers.
            assert!(
                !reason.contains("could not be sent"),
                "{failure:?} may have been written, so its reason must not claim otherwise: {reason}"
            );
        }
    }
}
