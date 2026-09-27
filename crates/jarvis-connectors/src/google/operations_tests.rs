//! Tests for the operation layer and the transport port.
//!
//! The load-bearing ones are about the **outcome boundary**: which transport failures may have reached the
//! provider, and therefore which refuse an automatic retry. That mapping is where an ambiguous failure becomes
//! a second effect if it is wrong, so each case is asserted with the consequence named rather than the variant
//! alone.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::google::credential::AccessToken;
use jarvis_core::ToolOutcome;
use serde_json::json;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn now() -> UtcTimestamp {
    UtcTimestamp::from_unix_nanos(1_774_000_000_500_000_000)
        .unwrap_or_else(|_| panic!("a representable instant"))
}

fn response(status: u16, body: &str) -> TransportResponse {
    TransportResponse {
        status,
        retry_after_seconds: None,
        body: body.to_owned(),
    }
}

#[test]
fn a_failure_that_may_have_reached_the_provider_is_never_reported_as_safe_to_retry() {
    // The most consequential mapping in this module, and the reason it is a table rather than a `bool`. Each
    // row asserts the class AND the consequence, because the class alone would still pass if the two variants
    // were swapped in a way that happened to satisfy one assertion.
    let cases = [
        (TransportFailure::Connect, false, "nothing was written"),
        (TransportFailure::Send, true, "the request was written"),
        (TransportFailure::Timeout, true, "it may have arrived"),
        (TransportFailure::Body, true, "the provider answered"),
        (
            TransportFailure::Refused {
                reason: "a forbidden redirect",
            },
            false,
            "this client refused before writing",
        ),
    ];
    for (failure, reached, why) in cases {
        assert_eq!(
            failure.may_have_reached_the_provider(),
            reached,
            "{failure:?} — {why}"
        );
        let error = failure_to_adapter_error(failure);
        if reached {
            assert!(
                matches!(error, AdapterError::AmbiguousAfterReaching { .. }),
                "{failure:?} must be ambiguous, not retryable: {error:?}"
            );
        } else {
            assert!(
                matches!(error, AdapterError::RefusedBeforeReaching { .. }),
                "{failure:?} must be a certain refusal: {error:?}"
            );
        }
    }
}

#[test]
fn a_transport_failure_never_becomes_a_provider_refusal() {
    // `AdapterError::ProviderRefused` means "the provider answered a refusal". A transport failure is by
    // definition NO answer, so using that variant would state that a provider decided something when nothing
    // was heard from it — a category error, and one that reads as if a decision had been made.
    for failure in [
        TransportFailure::Connect,
        TransportFailure::Send,
        TransportFailure::Body,
        TransportFailure::Timeout,
        TransportFailure::Refused {
            reason: "an off-allowlist host",
        },
    ] {
        let error = failure_to_adapter_error(failure);
        assert!(
            !matches!(error, AdapterError::ProviderRefused { .. }),
            "{failure:?} is not a provider's answer: {error:?}"
        );
    }
}

#[test]
fn a_provider_refusal_is_a_result_and_not_an_error() {
    // The other half of the boundary. A 403 is an ANSWER — the request was received and refused — so the adapter
    // reports a result. Returning an error would lose the status and the reason code, which is everything a
    // caller needs to decide what to do.
    let body = r#"{"error":{"code":403,"errors":[{"reason":"domainPolicy"}]}}"#;
    let result = must(
        interpret("google.gmail_messages_list", Ok(response(403, body)), now()),
        "a refusal must be reported rather than refused",
    );
    assert_eq!(result.outcome(), ToolOutcome::Failed);
    let reason = result.record().reason().unwrap_or_default();
    assert!(
        reason.contains("domainPolicy"),
        "the reason must be the provider's machine-readable code: {reason}"
    );
    // And it must NOT be the provider's prose, because a success-shaped string is not proof and `P3-008c`
    // forbids deriving a decision from message text.
    assert!(
        !reason.contains("\"error\""),
        "the outcome must not carry the raw body: {reason}"
    );
    assert!(
        result.evidence().is_none(),
        "a refusal produced no effect to locate"
    );
    assert!(result.output().is_none());
}

#[test]
fn a_refusal_with_an_unreadable_body_still_names_the_status() {
    // The body may be HTML from a proxy, a truncated stream, or something else entirely. The status is still a
    // fact, so the reason names it rather than the call becoming an error — the difference between "we know
    // little" and "we know nothing".
    let result = must(
        interpret(
            "google.gmail_messages_list",
            Ok(response(503, "<html>nope")),
            now(),
        ),
        "an unreadable refusal body must still be a result",
    );
    assert_eq!(result.outcome(), ToolOutcome::Failed);
    let reason = result.record().reason().unwrap_or_default();
    assert!(reason.contains("503"), "{reason}");
    assert!(
        !reason.contains("html"),
        "the body must not be carried: {reason}"
    );
}

#[test]
fn a_successful_list_becomes_a_confirmed_result_with_the_declared_output_shape() {
    // The output must match the tool's **own declared schema** — `message_ids` and `next_page_token` — rather
    // than Gmail's `Message` resource, or the schema would be a fiction (`ADR-0059`).
    let body = r#"{"messages":[{"id":"m1"},{"id":"m2"}],"nextPageToken":"more"}"#;
    let result = must(
        interpret("google.gmail_messages_list", Ok(response(200, body)), now()),
        "a successful list must be a result",
    );
    assert_eq!(result.outcome(), ToolOutcome::Confirmed);
    let output = result
        .output()
        .unwrap_or_else(|| panic!("a confirmed read must carry output"));
    let parsed: serde_json::Value = match serde_json::from_str(output.content()) {
        Ok(value) => value,
        Err(error) => panic!("the output must be JSON: {error}"),
    };
    assert_eq!(parsed["message_ids"][0], "m1");
    assert_eq!(parsed["message_ids"][1], "m2");
    assert_eq!(parsed["next_page_token"], "more");
    // The provider evidence for a read is the answer itself, and `confirmed` requires non-empty evidence — which
    // is asserted rather than assumed, because an empty rendering would make the outcome unrecordable.
    assert!(result.record().evidence().is_some());
}

#[test]
fn a_successful_response_whose_body_is_unreadable_is_unknown_and_not_confirmed() {
    // The case a naive adapter gets wrong. The status says the request was answered; the body says nothing about
    // what it produced. Reporting `Confirmed` would claim an effect on the strength of a status code, and
    // reporting `Failed` would claim nothing happened — `Unknown` is the only honest reading, and it is a
    // **result** rather than an error because the provider did answer.
    let result = must(
        interpret(
            "google.gmail_messages_list",
            Ok(response(200, "not json")),
            now(),
        ),
        "an unreadable success must still be a result",
    );
    assert_eq!(result.outcome(), ToolOutcome::Unknown);
    assert!(result.output().is_none());
    // And `Unknown` refuses an automatic retry, which is the consequence that matters.
    assert!(!result.outcome().is_safe_to_repeat_from_outcome());
}

#[test]
fn a_calendar_page_carries_both_tokens_separately() {
    // `nextSyncToken` and `nextPageToken` are different things: the sync token is present only on the last page
    // and positions a **future** incremental sync, while the page token continues the current walk. Merging them
    // would store a cursor that expires with the walk.
    let body = r#"{"items":[{"id":"e1"}],"nextPageToken":"page-1","nextSyncToken":"sync-1"}"#;
    let result = must(
        interpret(
            "google.calendar_events_read",
            Ok(response(200, body)),
            now(),
        ),
        "a successful events read must be a result",
    );
    let output = result
        .output()
        .unwrap_or_else(|| panic!("a confirmed read must carry output"));
    let parsed: serde_json::Value = match serde_json::from_str(output.content()) {
        Ok(value) => value,
        Err(error) => panic!("the output must be JSON: {error}"),
    };
    assert_eq!(parsed["next_page_token"], "page-1");
    assert_eq!(parsed["next_sync_token"], "sync-1");
    assert_ne!(parsed["next_page_token"], parsed["next_sync_token"]);
}

#[test]
fn an_unimplemented_tool_is_refused_before_any_request_is_built() {
    // `NotImplemented` rather than a default, because a fallback would make a mistyped name silently read a
    // mailbox — the same reasoning `jarvis-tools`' filesystem adapter records.
    let error = request_for("google.gmail_attachments_read", &json!({}))
        .err()
        .unwrap_or_else(|| panic!("an unknown tool must be refused"));
    assert!(matches!(error, OperationError::UnknownTool { .. }));
    assert!(matches!(
        interpret(
            "google.gmail_attachments_read",
            Ok(response(200, "{}")),
            now()
        ),
        Err(AdapterError::NotImplemented { .. })
    ));
}

#[test]
fn the_request_builder_reads_a_qualified_and_a_bare_name_alike() {
    // The matcher works on the **name segment**, so the pipeline's canonical identifier and a bare name both
    // resolve. Asserted because comparing the qualified form would be a second place the namespace is stated,
    // and a rename would then need two edits.
    for tool in ["google.gmail_messages_list", "gmail_messages_list"] {
        let request = must(request_for(tool, &json!({ "query": "is:unread" })), tool);
        assert_eq!(request.query().len(), 1);
        assert_eq!(request.query()[0].0, "q");
    }
}

#[test]
fn an_argument_that_is_supplied_but_wrong_is_refused_rather_than_dropped() {
    // The defect `P5-004` records from the other direction: a value that is present and ignored is
    // indistinguishable from one that was honoured, so a caller believes a bound was applied when it was not.
    assert!(request_for("google.gmail_messages_list", &json!({ "query": 42 })).is_err());
    assert!(
        request_for(
            "google.gmail_messages_list",
            &json!({ "max_results": "10" })
        )
        .is_err()
    );
    assert!(request_for("google.gmail_messages_list", &json!({ "page_token": 7 })).is_err());
    assert!(request_for("google.gmail_messages_read", &json!({ "format": "raw" })).is_err());
    // A null is an ABSENT value rather than a wrong one, which is what a JSON serializer emits for an unset
    // optional field — so it must be accepted.
    assert!(request_for("google.gmail_messages_list", &json!({ "query": null })).is_ok());
}

#[test]
fn a_read_that_names_a_resource_requires_it() {
    assert!(request_for("google.gmail_messages_read", &json!({})).is_err());
    assert!(request_for("google.gmail_messages_read", &json!({ "message_id": "" })).is_err());
    assert!(request_for("google.calendar_events_read", &json!({})).is_err());
    // The positive control: with the identifier present the request builds, so the refusal above is about the
    // missing name rather than about the operation being refused outright.
    assert!(request_for("google.gmail_messages_read", &json!({ "message_id": "m1" })).is_ok());
}

#[test]
fn the_message_format_defaults_to_full_and_never_to_raw() {
    // `full` is the schema's documented default, and `raw` is not expressible in the request layer at all — so
    // an absent format and an explicit `full` are the same request, while an unknown value is refused.
    let defaulted = must(
        request_for("google.gmail_messages_read", &json!({ "message_id": "m1" })),
        "an absent format is the default",
    );
    let explicit = must(
        request_for(
            "google.gmail_messages_read",
            &json!({ "message_id": "m1", "format": "full" }),
        ),
        "an explicit full format",
    );
    assert_eq!(defaulted.url_with_query(), explicit.url_with_query());
    assert!(defaulted.url_with_query().contains("format=full"));
    // And no input can produce `raw`.
    for value in ["raw", "RAW", "minimal ", "metadata-full"] {
        assert!(
            request_for(
                "google.gmail_messages_read",
                &json!({ "message_id": "m1", "format": value })
            )
            .is_err(),
            "`{value}` must not be accepted as a format"
        );
    }
}

/// A transport that answers with a scripted response, so the adapter is exercised without a socket.
struct Scripted {
    result: Result<TransportResponse, TransportFailure>,
}

impl GoogleTransport for Scripted {
    fn send(
        &self,
        _method: HttpMethod,
        _request: &crate::google::request::HttpRequest,
        _token: &AccessToken,
    ) -> Result<TransportResponse, TransportFailure> {
        self.result.clone()
    }
}

#[test]
fn the_read_tool_drives_a_transport_and_reports_what_it_established() {
    // The end-to-end shape: arguments become a request, the transport answers, and the answer becomes a result.
    // This is also the first caller of `GoogleTransport`, so the port is exercised rather than merely declared.
    let ok = Scripted {
        result: Ok(response(200, r#"{"messages":[{"id":"m1"}]}"#)),
    };
    let tool = GoogleReadTool::new(&ok);
    let token = must(
        AccessToken::new("ya29.a0AfH6SMBsecretvalue1234567890"),
        "a valid token",
    );
    let result = must(
        tool.execute(
            "google.gmail_messages_list",
            &json!({ "query": "is:unread" }),
            &token,
            now(),
        ),
        "a successful read must report a result",
    );
    assert_eq!(result.outcome(), ToolOutcome::Confirmed);

    // And a failure that may have reached the provider is not reported as safe to retry, end to end.
    let ambiguous = Scripted {
        result: Err(TransportFailure::Timeout),
    };
    let tool = GoogleReadTool::new(&ambiguous);
    let error = tool
        .execute("google.gmail_messages_list", &json!({}), &token, now())
        .err()
        .unwrap_or_else(|| panic!("a timeout must be ambiguous"));
    assert!(matches!(error, AdapterError::AmbiguousAfterReaching { .. }));

    // While a certain refusal before writing is reported as one, so the retry IS permitted.
    let certain = Scripted {
        result: Err(TransportFailure::Connect),
    };
    let tool = GoogleReadTool::new(&certain);
    let error = tool
        .execute("google.gmail_messages_list", &json!({}), &token, now())
        .err()
        .unwrap_or_else(|| panic!("a connect failure must be a refusal"));
    assert!(matches!(error, AdapterError::RefusedBeforeReaching { .. }));
}

#[test]
fn the_transport_port_never_reports_a_provider_refusal_as_a_failure() {
    // A non-2xx from the transport is a response the adapter classifies, not a transport failure. Collapsing
    // them would lose the status and the reason, which is everything `classify` needs — and the `is_success`
    // predicate is the one place the distinction is made, so it is asserted for both sides.
    assert!(response(200, "{}").is_success());
    for status in [201, 204, 301, 400, 401, 403, 404, 429, 500, 503] {
        assert!(
            !response(status, "{}").is_success(),
            "{status} must not read as a success"
        );
    }
}

#[test]
fn evidence_is_optional_for_a_read_so_a_bad_locator_does_not_fail_a_good_read() {
    // A malformed locator header must not turn a successful read into an error: the read succeeded, and failing
    // it because a header was unusable would be a worse outcome than carrying no locator.
    assert!(evidence_from(None).is_none());
    assert!(evidence_from(Some("")).is_none());
    assert!(evidence_from(Some("   ")).is_none());
    let evidence =
        evidence_from(Some("req-123")).unwrap_or_else(|| panic!("a locator must be carried"));
    assert_eq!(evidence.as_str(), "req-123");
}

#[test]
fn the_method_comes_from_the_request_and_the_port_expresses_only_a_get() {
    // All three declared operations are reads, so the port's method set has one member. `P5-009` must extend
    // both together, which is what makes a write a deliberate edit rather than a convenience.
    let request = must(
        request_for("google.gmail_messages_list", &json!({})),
        "a read request",
    );
    assert_eq!(must(method_for(&request), "a GET"), HttpMethod::Get);
    assert_eq!(HttpMethod::Get.as_str(), "GET");
    assert_eq!(HttpMethod::Get.to_string(), "GET");
}
