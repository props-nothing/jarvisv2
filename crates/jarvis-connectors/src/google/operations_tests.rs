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
        retry_after: None,
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
fn a_refusal_carries_the_retry_class_and_the_stated_delay() {
    // **`client::classify` had no production caller until this test's subject was wired in.** The table is the
    // one `ADR-0058` exists for — four reasons share a `403` with three remedies, so a status-only classifier
    // retries an administrator's decision forever — and its output went nowhere: `refusal` read the reason code,
    // formatted it, and dropped the class. A `429`'s `Retry-After` was carried by the transport and then ignored
    // here. So these cases assert the two facts a caller needs and could not previously see.
    let throttled = must(
        interpret(
            "google.gmail_messages_list",
            Ok(TransportResponse {
                status: 429,
                // The provider's own stated delay, which the transport carries and never interprets.
                retry_after: Some(crate::google::transport::RetryAfter::Seconds(37)),
                body: r#"{"error":{"code":429,"errors":[{"reason":"rateLimitExceeded"}]}}"#
                    .to_owned(),
            }),
            now(),
        ),
        "a 429 must be a result",
    );
    let reason = throttled.record().reason().unwrap_or_default();
    assert!(
        reason.contains("throttled"),
        "the class must be readable, not only the code: {reason}"
    );
    assert!(
        reason.contains("retry after 37s"),
        "a stated delay must reach the reason: {reason}"
    );

    // A `domainPolicy` 403 is the case the whole table exists for: the same status as the two throttling
    // reasons, and a remedy that is a conversation rather than a retry. It must be `permanent` and carry **no**
    // delay — a caller that saw seconds here would back off and retry an administrator's decision.
    let disabled = must(
        interpret(
            "google.gmail_messages_list",
            Ok(TransportResponse {
                status: 403,
                // Even a `Retry-After` present on the wire must not produce a delay for a permanent class:
                // `RetryGuidance`'s permanent arms carry no seconds, so there is nothing to append.
                retry_after: Some(crate::google::transport::RetryAfter::Seconds(60)),
                body: r#"{"error":{"code":403,"errors":[{"reason":"domainPolicy"}]}}"#.to_owned(),
            }),
            now(),
        ),
        "a 403 must be a result",
    );
    let reason = disabled.record().reason().unwrap_or_default();
    assert!(reason.contains("domainPolicy"), "{reason}");
    assert!(
        reason.contains("permanent"),
        "the class must say the refusal is not retryable: {reason}"
    );
    assert!(
        !reason.contains("retry after"),
        "a permanent refusal must not carry a delay, even when the wire had one: {reason}"
    );

    // And a 5xx is a provider fault with a backoff floor, which is the third remedy among the three.
    let fault = must(
        interpret(
            "google.gmail_messages_list",
            Ok(response(503, r#"{"error":{"code":503}}"#)),
            now(),
        ),
        "a 503 must be a result",
    );
    let reason = fault.record().reason().unwrap_or_default();
    assert!(reason.contains("provider_fault"), "{reason}");
    assert!(reason.contains("retry after"), "{reason}");
}

#[test]
fn a_stated_delay_above_the_ceiling_reaches_the_reason_as_a_deferral() {
    // **The bound `MAX_RETRY_AFTER_SECONDS` documents, asserted through the production path.** Its doc says a
    // longer value is "refused rather than clamped", and until this test the only thing that knew that was the
    // comment: `classify` passed the provider's number straight into `RetryAfterSeconds`, so a `429` stating
    // `Retry-After: 18000` produced a reason reading "retry after 18000s" — a five-hour wait presented as an
    // ordinary retry delay, which is precisely the loop the bound exists to prevent. Google documents that a
    // daily-limit 429 "might result in these errors for multiple hours", so the input is realistic.
    let deferred = must(
        interpret(
            "google.gmail_messages_list",
            Ok(TransportResponse {
                status: 429,
                retry_after: Some(crate::google::transport::RetryAfter::Seconds(18_000)),
                body: r#"{"error":{"code":429}}"#.to_owned(),
            }),
            now(),
        ),
        "a 429 must be a result",
    );
    let reason = deferred.record().reason().unwrap_or_default();
    assert!(
        reason.contains("defer for 18000s"),
        "an over-ceiling delay must read as a deferral: {reason}"
    );
    assert!(
        !reason.contains("retry after"),
        "a deferral must not read as an automatic retry: {reason}"
    );
    // The class is still `Throttled`, because it is — the *remedy* is what changes, not the diagnosis. A
    // reader who saw `unknown` or `permanent` here would reach for the wrong recovery action.
    assert!(
        reason.contains("throttled"),
        "the diagnosis must survive the deferral: {reason}"
    );

    // The control: the same status one second **inside** the ceiling stays an ordinary stated delay. Without
    // it, a `classify` that deferred every 429 would pass the assertions above.
    let inside = must(
        interpret(
            "google.gmail_messages_list",
            Ok(TransportResponse {
                status: 429,
                retry_after: Some(crate::google::transport::RetryAfter::Seconds(
                    MAX_RETRY_AFTER_SECONDS,
                )),
                body: r#"{"error":{"code":429}}"#.to_owned(),
            }),
            now(),
        ),
        "a 429 must be a result",
    );
    let reason = inside.record().reason().unwrap_or_default();
    assert!(
        reason.contains(&format!("retry after {MAX_RETRY_AFTER_SECONDS}s")),
        "a delay within the ceiling must retry: {reason}"
    );
    assert!(
        !reason.contains("defer"),
        "the control must not defer: {reason}"
    );
}

#[test]
fn a_calendar_410_reaches_the_reason_as_a_dead_cursor_rather_than_an_unknown() {
    // **The end-to-end consequence of the API-aware classifier.** A `410` from `calendar_events_read` is the
    // dead-sync-token state Calendar documents, so a caller reading a stored failed call must see a definite
    // verdict — not `unknown`, which means "establish what happened before doing anything else" and would send
    // an operator hunting for a cause the provider already named.
    let dead_cursor = must(
        interpret(
            "google.calendar_events_read",
            Ok(response(410, r#"{"error":{"code":410}}"#)),
            now(),
        ),
        "a 410 must be a result",
    );
    let reason = dead_cursor.record().reason().unwrap_or_default();
    assert!(
        reason.contains("permanent"),
        "a Calendar 410 is a known, definite state: {reason}"
    );
    assert!(
        !reason.contains("unknown"),
        "the defect this fixed: a documented 410 must not read as unclassified: {reason}"
    );

    // The control, and the one that makes the assertion above about the API rather than about the status: the
    // SAME status on a Gmail operation stays unclassified, because Gmail's error page documents no 410.
    let gmail_410 = must(
        interpret(
            "google.gmail_messages_list",
            Ok(response(410, r#"{"error":{"code":410}}"#)),
            now(),
        ),
        "a 410 must be a result",
    );
    let reason = gmail_410.record().reason().unwrap_or_default();
    assert!(
        reason.contains("unknown"),
        "Gmail documents no 410, so it stays the fail-closed answer: {reason}"
    );
}

#[test]
fn an_unreadable_refusal_is_classified_by_its_status_and_not_guessed() {
    // A body that cannot be parsed still has a status, and the status is what `classify` switches on when there
    // is no reason. So the class is present either way — the difference between knowing little and knowing
    // nothing, applied to the retry decision rather than to the fact of the refusal.
    let result = must(
        interpret(
            "google.gmail_messages_list",
            Ok(response(503, "<html>a proxy</html>")),
            now(),
        ),
        "an unreadable refusal is still a result",
    );
    let reason = result.record().reason().unwrap_or_default();
    assert!(
        reason.contains("provider_fault"),
        "a 5xx stays a provider fault without a reason code: {reason}"
    );
    // The unclassified status is the fail-closed direction: `Unknown` refuses a retry whatever the idempotency,
    // so an unrecognised status must not read as transient.
    let odd = must(
        interpret("google.gmail_messages_list", Ok(response(418, "{}")), now()),
        "an unrecognised status is still a result",
    );
    let reason = odd.record().reason().unwrap_or_default();
    assert!(
        reason.contains("unknown"),
        "an unclassified status must say so: {reason}"
    );
    assert!(
        !reason.contains("retry after"),
        "`Reconcile` carries no delay, because a retry is not the action: {reason}"
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

/// Renders one Calendar page and returns the fields the tool declared.
///
/// A helper because the two pages below differ **only** in which continuation token they carry, and Google's
/// documentation states the two are mutually exclusive: `nextPageToken` is "omitted if no further results are
/// available, in which case nextSyncToken is provided", and `nextSyncToken` is "omitted if further results are
/// available, in which case nextPageToken is provided". So a body carrying both is one the provider cannot
/// produce, and a fixture that used one would be asserting behaviour against an impossible input — which
/// **the first version of this test did**. See the fixtures under `tests/fixtures/google/` for the recorded
/// shapes.
fn calendar_page_fields(body: &str) -> serde_json::Value {
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
    must(
        serde_json::from_str(output.content()),
        "the output must be JSON",
    )
}

#[test]
fn a_mid_walk_calendar_page_carries_a_page_token_and_no_sync_token() {
    // The sync token marks the **end** of a walk, so a page that reports more results cannot carry one. A caller
    // that treated a page token as a cursor would store a token that expires when the walk finishes — which is
    // the defect this distinction exists to prevent, and it is invisible unless the two states are tested apart.
    let fields =
        calendar_page_fields(r#"{"items":[{"id":"e1"},{"id":"e2"}],"nextPageToken":"page-1"}"#);
    assert_eq!(fields["event_ids"][0], "e1");
    assert_eq!(fields["event_ids"][1], "e2");
    assert_eq!(fields["next_page_token"], "page-1");
    assert_eq!(
        fields["next_sync_token"],
        serde_json::Value::Null,
        "a page with more results must not carry a sync token"
    );
}

#[test]
fn a_final_calendar_page_carries_a_sync_token_and_no_page_token() {
    // The last page is the only place a cursor may be taken from, and it reports no page token. Both halves are
    // asserted, so a change that produced one token from the other would fail here.
    let fields = calendar_page_fields(r#"{"items":[{"id":"e3"}],"nextSyncToken":"sync-1"}"#);
    assert_eq!(fields["event_ids"][0], "e3");
    assert_eq!(
        fields["next_page_token"],
        serde_json::Value::Null,
        "the last page must not carry a page token"
    );
    assert_eq!(fields["next_sync_token"], "sync-1");
}

/// Validates a rendered output against the schema the definition declares for that tool.
///
/// The schema is **derived from the manifest and not restated**, so this joins the two halves that were only
/// ever asserted separately: the rendering in this module and the contract `ADR-0059` derives. Nothing did,
/// which is why a divergence between them would have been found by a reader rather than by a test.
fn assert_output_matches_declared_schema(tool: &str, output: &str) {
    let manifest = must(
        super::super::GoogleConnector::manifest(),
        "the manifest must be built",
    );
    let definition = must(
        crate::google::definitions::definitions(&manifest),
        "the manifest must derive its definitions",
    )
    .into_iter()
    .find(|candidate| candidate.id().to_string() == tool)
    .unwrap_or_else(|| panic!("the manifest must declare {tool}"));
    let value: serde_json::Value = must(
        serde_json::from_str(output),
        "a rendered output must be JSON",
    );
    let report = must(
        definition.output_schema().validate(&value),
        "the declared schema must be usable",
    );
    assert!(
        report.is_valid(),
        "the rendering of {tool} must satisfy its own declared schema, violations: {:?}",
        report.violations()
    );
}

/// Returns the declared top-level output property names of a tool, from its own schema.
fn declared_output_properties(tool: &str) -> Vec<String> {
    let manifest = must(
        super::super::GoogleConnector::manifest(),
        "the manifest must be built",
    );
    let definition = must(
        crate::google::definitions::definitions(&manifest),
        "the manifest must derive its definitions",
    )
    .into_iter()
    .find(|candidate| candidate.id().to_string() == tool)
    .unwrap_or_else(|| panic!("the manifest must declare {tool}"));
    let mut names: Vec<String> = definition
        .output_schema()
        .document()
        .pointer("/properties")
        .and_then(serde_json::Value::as_object)
        .map_or_else(
            || panic!("{tool} must declare a `properties` object"),
            |properties| properties.keys().cloned().collect(),
        );
    names.sort();
    names
}

/// Returns the top-level keys a rendered output actually carries.
fn rendered_keys(output: &str) -> Vec<String> {
    let value: serde_json::Value = must(serde_json::from_str(output), "a rendering must be JSON");
    let mut keys: Vec<String> = value.as_object().map_or_else(
        || panic!("a rendering must be an object: {output}"),
        |object| object.keys().cloned().collect(),
    );
    keys.sort();
    keys
}

#[test]
fn a_read_delivers_every_field_its_output_declares() {
    // **The forward direction of `ADR-0083`'s finding.** `gmail_messages_read` declared four output fields and
    // its renderer emitted one, so three quarters of the declared contract was unreachable — a consumer
    // reading a field the schema promised would find nothing. This asserts the fields `full` returns are all
    // rendered, so the declaration is not a promise the code declines to keep.
    let message = must(
        interpret(
            "google.gmail_messages_read",
            Ok(response(
                200,
                r#"{"id":"m1","threadId":"t1","labelIds":["INBOX","UNREAD"]}"#,
            )),
            now(),
        ),
        "a read must be a result",
    );
    let output = message
        .output()
        .unwrap_or_else(|| panic!("a confirmed read carries output"))
        .content();
    assert_output_matches_declared_schema("google.gmail_messages_read", output);
    let keys = rendered_keys(output);
    assert!(keys.contains(&"message_id".to_owned()), "{output}");
    assert!(keys.contains(&"thread_id".to_owned()), "{output}");
    assert!(keys.contains(&"label_ids".to_owned()), "{output}");
}

#[test]
fn no_declared_output_property_is_undeliverable() {
    // **The reverse direction, and the one that finds the defect the forward test cannot.** The forward test
    // proves the fields the renderer *emits* are declared; only this proves every field the schema *declares* is
    // something the renderer can produce. A field added to the schema and forgotten in the renderer passes every
    // other test — the schema validates, nothing references the field — and fails here, because a maximal
    // response does not produce a matching key. That is exactly how `thread_id`, `label_ids` and `snippet` were
    // declared and never rendered (`ADR-0083`).
    //
    // The comparison is between the declared property NAMES and the keys of the rendering, so it cannot be
    // satisfied by a schema the renderer ignores. `snippet` is the case that made this necessary: it was
    // declared and is genuinely undeliverable, which is why it was **removed from the schema** rather than left
    // to be found missing here.
    let maximal = must(
        interpret(
            "google.gmail_messages_read",
            Ok(response(
                200,
                r#"{"id":"m1","threadId":"t1","labelIds":["INBOX"]}"#,
            )),
            now(),
        ),
        "a read must be a result",
    );
    let output = maximal
        .output()
        .unwrap_or_else(|| panic!("a confirmed read carries output"))
        .content();
    assert_eq!(
        rendered_keys(output),
        declared_output_properties("google.gmail_messages_read"),
        "every declared output property must be producible from a maximal response, and nothing rendered \
         may be undeclared: {output}"
    );
}

#[test]
fn a_response_without_thread_or_labels_omits_them_rather_than_inventing_an_empty_value() {
    // The two optional fields are omitted when the provider did not return them — `minimal` and `metadata` may
    // not include `threadId`, and the Format page says nothing about it for either. The distinction that
    // matters is `label_ids`: absent means "the provider did not return labels", while `[]` means "the message
    // carries no labels". Rendering `[]` for an absent field would turn the first into the second, so the keys
    // are asserted **absent** rather than merely empty (`ADR-0083`).
    let sparse = must(
        interpret(
            "google.gmail_messages_read",
            Ok(response(200, r#"{"id":"m1"}"#)),
            now(),
        ),
        "a read must be a result",
    );
    let output = sparse
        .output()
        .unwrap_or_else(|| panic!("a confirmed read carries output"))
        .content();
    // It still satisfies the schema, which is the point: an honest response with only the required field is
    // valid, so a `minimal` read is not forced to fabricate fields it does not have.
    assert_output_matches_declared_schema("google.gmail_messages_read", output);
    let keys = rendered_keys(output);
    assert_eq!(
        keys,
        vec!["message_id"],
        "only the required field is present: {output}"
    );

    // The control: an explicit empty label list **is** rendered, so the omission above is driven by the
    // field's absence and not by the renderer dropping every empty value.
    let empty_labels = must(
        interpret(
            "google.gmail_messages_read",
            Ok(response(200, r#"{"id":"m1","labelIds":[]}"#)),
            now(),
        ),
        "a read must be a result",
    );
    let output = empty_labels
        .output()
        .unwrap_or_else(|| panic!("a confirmed read carries output"))
        .content();
    assert!(
        output.contains(r#""label_ids":[]"#),
        "an explicit empty label list must be rendered, not dropped: {output}"
    );
}

#[test]
fn every_rendered_output_satisfies_the_schema_the_definition_declares() {
    // The claim `ADR-0059` makes is that the tool's declared output is what this module renders, and until now
    // that was a comment rather than a check. One case per operation, so a field renamed on one side
    // fails here — the "two values that must agree, with nothing holding both" defect this repository keeps
    // recording.
    let list = must(
        interpret(
            "google.gmail_messages_list",
            Ok(response(
                200,
                r#"{"messages":[{"id":"m1"}],"nextPageToken":"more"}"#,
            )),
            now(),
        ),
        "a list must be a result",
    );
    assert_output_matches_declared_schema(
        "google.gmail_messages_list",
        list.output()
            .unwrap_or_else(|| panic!("a confirmed read carries output"))
            .content(),
    );

    let single = must(
        interpret(
            "google.gmail_messages_read",
            Ok(response(200, r#"{"id":"m1"}"#)),
            now(),
        ),
        "a read must be a result",
    );
    assert_output_matches_declared_schema(
        "google.gmail_messages_read",
        single
            .output()
            .unwrap_or_else(|| panic!("a confirmed read carries output"))
            .content(),
    );

    let calendar = must(
        interpret(
            "google.calendar_events_read",
            Ok(response(
                200,
                r#"{"items":[{"id":"e1"}],"nextSyncToken":"s"}"#,
            )),
            now(),
        ),
        "an events read must be a result",
    );
    assert_output_matches_declared_schema(
        "google.calendar_events_read",
        calendar
            .output()
            .unwrap_or_else(|| panic!("a confirmed read carries output"))
            .content(),
    );

    // The history read renders **two** tokens, and the declared schema must accept both together: the page
    // token continues this walk while the history id is the durable position, so a rendering that dropped one
    // would lose half of what a sync needs.
    let history = must(
        interpret(
            "google.gmail_history_list",
            Ok(response(
                200,
                r#"{"history":[{"messages":[{"id":"m1"}]}],"nextPageToken":"p","historyId":"12347"}"#,
            )),
            now(),
        ),
        "a history read must be a result",
    );
    let history_output = history
        .output()
        .unwrap_or_else(|| panic!("a confirmed read carries output"))
        .content();
    assert_output_matches_declared_schema("google.gmail_history_list", history_output);
    assert!(
        history_output.contains("\"history_id\":\"12347\""),
        "the durable cursor must be rendered as its own field: {history_output}"
    );

    // And the empty-page case, which is where a rendering that omitted a required array would appear: an
    // omitted `message_ids` and an empty `message_ids` are different documents, and only the second satisfies
    // the schema's `required`.
    let empty = must(
        interpret("google.gmail_messages_list", Ok(response(200, "{}")), now()),
        "an empty page is still a result",
    );
    let rendered = empty
        .output()
        .unwrap_or_else(|| panic!("a confirmed read carries output"))
        .content();
    assert_output_matches_declared_schema("google.gmail_messages_list", rendered);
    assert!(rendered.contains("\"message_ids\":[]"), "{rendered}");
}

#[test]
fn the_profile_read_renders_the_identity_every_declared_field_of_it() {
    // **The identity read end to end through the operation layer**, which is the layer `ADR-0083` found a gap
    // in: an output schema promised fields the renderer never produced. Here the schema declares two, so both
    // must be asserted — the address always, and the position when the provider sent one.
    let result = must(
        interpret(
            "google.gmail_profile_read",
            Ok(response(
                200,
                r#"{"emailAddress":"user@example.com","messagesTotal":42,"threadsTotal":7,"historyId":"1234567890"}"#,
            )),
            now(),
        ),
        "a profile read must be a result",
    );
    let output = result
        .output()
        .unwrap_or_else(|| panic!("a confirmed read carries output"))
        .content();
    assert_output_matches_declared_schema("google.gmail_profile_read", output);
    assert!(
        output.contains("\"email_address\":\"user@example.com\""),
        "the declared identity field must be rendered: {output}"
    );
    assert!(
        output.contains("\"history_id\":\"1234567890\""),
        "the declared position field must be rendered: {output}"
    );
    // **The two fields this connector does not declare are not rendered.** `messagesTotal` and `threadsTotal`
    // are mailbox counts the connector's output schema deliberately omits, and rendering them anyway would
    // reintroduce `ADR-0083`'s defect in the other direction — a value in the output that no declared field
    // promises.
    assert!(
        !output.contains("messagesTotal") && !output.contains("threadsTotal"),
        "the output must carry only the fields the schema declares: {output}"
    );

    // And the identity-only case: `history_id` is optional in the schema, so a profile without one must still
    // validate rather than failing a `required` check it never promised.
    let identity_only = must(
        interpret(
            "google.gmail_profile_read",
            Ok(response(200, r#"{"emailAddress":"user@example.com"}"#)),
            now(),
        ),
        "an identity-only profile is still a result",
    );
    let rendered = identity_only
        .output()
        .unwrap_or_else(|| panic!("a confirmed read carries output"))
        .content();
    assert_output_matches_declared_schema("google.gmail_profile_read", rendered);
    assert!(
        !rendered.contains("history_id"),
        "an absent position must be omitted rather than rendered as null: {rendered}"
    );
}

#[test]
fn a_sync_token_with_a_time_range_is_refused_by_the_operation_layer() {
    // The operation layer is what a model's arguments pass through, so this is where the impossible pairing
    // must be stopped — by `request_for`, **before** a transport call is built. The refusal is a
    // `RefusedBeforeReaching` style fault, not a provider answer, because nothing was sent: the point of
    // catching it here is that a `400` from Google would cost a request and still not tell the caller which
    // argument to drop (`ADR-0084`).
    let error = request_for(
        "google.calendar_events_read",
        &json!({ "calendar_id": "primary", "sync_token": "tok", "time_min": "2026-01-01T00:00:00Z" }),
    )
    .err()
    .unwrap_or_else(|| panic!("a time range with a sync token must be refused"));
    assert!(
        matches!(
            error,
            OperationError::Request(RequestError::DisallowedCombination { .. })
        ),
        "the refusal must survive the operation layer as a combination fault: {error:?}"
    );

    // The same call without the time bound builds, so the refusal is about the pairing and not the token.
    assert!(
        request_for(
            "google.calendar_events_read",
            &json!({ "calendar_id": "primary", "sync_token": "tok" }),
        )
        .is_ok()
    );
}

#[test]
fn a_calendar_page_token_is_accepted_and_sent_by_the_operation_layer() {
    // The input the output's `next_page_token` needs. Before this the renderer emitted a page token the caller
    // was told to use and **no argument existed to use it with**, so a large read could not be continued — and a
    // large *incremental* sync is the normal case, since the provider returns a page token instead of a sync
    // token mid-walk (`ADR-0085`).
    let request = must(
        request_for(
            "google.calendar_events_read",
            &json!({ "calendar_id": "primary", "sync_token": "sync-1", "page_token": "page-2" }),
        ),
        "a page token alongside a sync token is the documented walk",
    );
    let query: Vec<&str> = request
        .query()
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    assert!(query.contains(&"pageToken"), "{query:?}");
    assert!(
        query.contains(&"syncToken"),
        "the sync token must survive alongside the page token: {query:?}"
    );
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
    // The history read names the position it starts from, and the reference marks it Required — so an absent
    // `start_history_id` is refused by argument rather than defaulting to "from the beginning of time".
    assert!(request_for("google.gmail_history_list", &json!({})).is_err());
    assert!(
        request_for(
            "google.gmail_history_list",
            &json!({ "start_history_id": "  " })
        )
        .is_err(),
        "a blank position addresses nothing"
    );
    // The positive control: with the identifier present the request builds, so the refusal above is about the
    // missing name rather than about the operation being refused outright.
    assert!(request_for("google.gmail_messages_read", &json!({ "message_id": "m1" })).is_ok());
    assert!(
        request_for(
            "google.gmail_history_list",
            &json!({ "start_history_id": "12345" })
        )
        .is_ok()
    );
}

#[test]
fn the_history_read_builds_its_request_and_reads_its_cursor_from_a_success() {
    // The operation is wired end to end: an argument object becomes a request, and a 200 becomes the declared
    // output with the durable cursor rendered separately from the page token.
    let request = must(
        request_for(
            "google.gmail_history_list",
            &json!({ "start_history_id": "12345", "max_results": 100 }),
        ),
        "a valid history request",
    );
    assert_eq!(
        request.url(),
        "https://www.googleapis.com/gmail/v1/users/me/history"
    );
    assert!(
        request.url_with_query().contains("startHistoryId=12345"),
        "the starting position must be sent: {}",
        request.url_with_query()
    );

    // A 404 is a RESULT, not an error: the provider answered and refused, and the caller turns that status
    // into `SyncSignal::CursorUnusable` through `client::gmail_history_signal`. Reporting it as a transport
    // failure would lose the status, which is the only fact the decision has.
    let refused = must(
        interpret(
            "google.gmail_history_list",
            Ok(response(404, r#"{"error":{"code":404}}"#)),
            now(),
        ),
        "a refusal is a result, not an error",
    );
    assert_eq!(refused.outcome(), ToolOutcome::Failed);
    assert!(refused.output().is_none());
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

#[async_trait::async_trait]
impl GoogleTransport for Scripted {
    async fn send(
        &self,
        _method: HttpMethod,
        _request: &crate::google::request::HttpRequest,
        _token: &AccessToken,
    ) -> Result<TransportResponse, TransportFailure> {
        self.result.clone()
    }

    async fn send_form(
        &self,
        _request: &crate::google::request::FormRequest,
    ) -> Result<TransportResponse, TransportFailure> {
        // This double scripts the **read** path, so a form `POST` reaching it is a mistake in the test rather
        // than a provider condition — the same reasoning the exchange tests record in the other direction.
        panic!("the read path must not use `send_form`");
    }
}

#[tokio::test]
async fn the_read_tool_drives_a_transport_and_reports_what_it_established() {
    // The end-to-end shape: arguments become a request, the transport answers, and the answer becomes a result.
    // This is also the first caller of `GoogleTransport`, so the port is exercised rather than merely declared.
    let ok = Scripted {
        result: Ok(response(200, r#"{"messages":[{"id":"m1"}]}"#)),
    };
    let token = must(
        AccessToken::new("ya29.a0AfH6SMBsecretvalue1234567890"),
        "a valid token",
    );
    let tool = GoogleReadTool::new(&ok, &token, "google-read");
    let result = must(
        tool.run(
            "google.gmail_messages_list",
            &json!({ "query": "is:unread" }),
            now(),
        )
        .await,
        "a successful read must report a result",
    );
    assert_eq!(result.outcome(), ToolOutcome::Confirmed);

    // And a failure that may have reached the provider is not reported as safe to retry, end to end.
    let ambiguous = Scripted {
        result: Err(TransportFailure::Timeout),
    };
    let tool = GoogleReadTool::new(&ambiguous, &token, "google-read");
    let error = tool
        .run("google.gmail_messages_list", &json!({}), now())
        .await
        .err()
        .unwrap_or_else(|| panic!("a timeout must be ambiguous"));
    assert!(matches!(error, AdapterError::AmbiguousAfterReaching { .. }));

    // While a certain refusal before writing is reported as one, so the retry IS permitted.
    let certain = Scripted {
        result: Err(TransportFailure::Connect),
    };
    let tool = GoogleReadTool::new(&certain, &token, "google-read");
    let error = tool
        .run("google.gmail_messages_list", &json!({}), now())
        .await
        .err()
        .unwrap_or_else(|| panic!("a connect failure must be a refusal"));
    assert!(matches!(error, AdapterError::RefusedBeforeReaching { .. }));
}

#[test]
fn the_adapter_reports_the_identifier_it_was_given() {
    // `adapter_id` names the adapter in a diagnostic, and the identifier is the pipeline's rather than a
    // constant here — so it is asserted to round-trip rather than assumed to be a literal.
    use jarvis_tools::ToolExecutor;
    let ok = Scripted {
        result: Ok(response(200, "{}")),
    };
    let token = must(
        AccessToken::new("ya29.a0AfH6SMBsecretvalue1234567890"),
        "valid",
    );
    assert_eq!(
        GoogleReadTool::new(&ok, &token, "google-read").adapter_id(),
        "google-read"
    );
    assert_eq!(
        GoogleReadTool::new(&ok, &token, "google-mail-read").adapter_id(),
        "google-mail-read"
    );
}

/// Builds a request the pipeline would have authorized for one Google read tool.
///
/// The authority is **real**: the digest is recomputed from the arguments the request carries and the decision
/// comes from an actual `evaluate` over the tool's own derived definition, because
/// `AuthorizationReceipt::new` and `ToolExecutionRequest::new` both verify everything. A fixture cannot stand
/// in an invented digest or a fabricated decision — that is what makes this a test of the adapter rather than
/// of a mock.
fn authorized(
    tool: &str,
    arguments: serde_json::Value,
    deadline: UtcTimestamp,
) -> jarvis_tools::ToolExecutionRequest {
    use jarvis_tools::{
        ActorAuthority, AuthenticationStrength, AuthorizationReceipt, AuthorizationReceiptParts,
        IdempotencyKey, PolicyRequest, Scope, ScopeSet, TargetAssessment, ToolExecutionRequest,
        ToolExecutionRequestParts, ToolId, WorkspacePolicy, evaluate,
    };
    let definition = must(
        crate::google::definitions::definitions(&must(
            super::super::GoogleConnector::manifest(),
            "the manifest must be built",
        )),
        "the manifest must derive its definitions",
    )
    .into_iter()
    .find(|candidate| candidate.id().to_string() == tool)
    .unwrap_or_else(|| panic!("the manifest must declare {tool}"));

    // The actor holds exactly the two scopes the manifest requires, so the decision is allowed by the
    // declared contract rather than by a wildcard that would also cover tools this connector does not have.
    let actor = ActorAuthority::active(ScopeSet::new([
        must(Scope::new("mail.read"), "a valid scope"),
        must(Scope::new("calendar.read"), "a valid scope"),
    ]));
    let workspace = WorkspacePolicy::default();
    let decision = evaluate(&PolicyRequest {
        definition: &definition,
        actor,
        workspace: &workspace,
        channel: jarvis_core::SessionChannel::Cli,
        claimed_strength: AuthenticationStrength::Present,
        available: true,
        target: TargetAssessment::none(),
    });
    assert!(
        decision.is_allowed(),
        "{tool} must be allowed for the fixture, got {:?}",
        decision.reason_code()
    );

    let intent_hash = must(
        jarvis_core::CanonicalIntentHash::compute(tool, "1.0.0", &arguments),
        "the arguments must be hashable",
    );
    let issued_at = UtcTimestamp::from_unix_nanos(1_774_000_000_000_000_000)
        .unwrap_or_else(|_| panic!("a representable instant"));
    let receipt = must(
        AuthorizationReceipt::new(AuthorizationReceiptParts {
            receipt_id: "0198f000-0000-7000-8000-0000000000f1".to_owned(),
            tool: must(ToolId::new(tool), "a valid tool id"),
            tool_version: "1.0.0".to_owned(),
            arguments: arguments.clone(),
            intent_hash,
            policy_version: "policy-3".to_owned(),
            decision,
            approval: None,
            correlation_id: jarvis_core::CorrelationId::new(),
            issued_at,
        }),
        "the receipt must be built",
    );
    must(
        ToolExecutionRequest::new(ToolExecutionRequestParts {
            call_id: "0198f000-0000-7000-8000-0000000000f3".to_owned(),
            tool: must(ToolId::new(tool), "a valid tool id"),
            tool_version: "1.0.0".to_owned(),
            arguments,
            receipt,
            idempotency_key: must(IdempotencyKey::generate(), "a key"),
            deadline,
            correlation_id: jarvis_core::CorrelationId::new(),
        }),
        "the request must be built",
    )
}

#[tokio::test]
async fn the_adapter_is_a_tool_executor_and_drives_a_request_the_pipeline_built() {
    // Why this test exists: the port was first written **synchronous**, and `ToolExecutor::execute` is `async`.
    // A synchronous port can never back an adapter for an async interface — an `async` function calling a
    // blocking one blocks a runtime worker for the whole round trip — so the shape was wrong and this test is
    // what would have caught it. It drives the real trait, not the helper.
    use jarvis_tools::ToolExecutor;

    let transport = Scripted {
        result: Ok(response(200, r#"{"messages":[{"id":"m1"}]}"#)),
    };
    let token = must(
        AccessToken::new("ya29.a0AfH6SMBsecretvalue1234567890"),
        "valid",
    );
    let adapter = GoogleReadTool::new(&transport, &token, "google-read");
    let request = authorized(
        "google.gmail_messages_list",
        json!({ "query": "is:unread" }),
        UtcTimestamp::from_unix_nanos(1_900_000_000_000_000_000)
            .unwrap_or_else(|_| panic!("a representable future instant")),
    );
    let result = must(
        adapter.execute(&request).await,
        "an authorized call must produce a result",
    );
    assert_eq!(result.outcome(), ToolOutcome::Confirmed);
}

#[tokio::test]
async fn a_call_already_past_its_deadline_is_refused_before_anything_is_sent() {
    // The deadline guard, and why it must be `RefusedBeforeReaching` rather than a failure: nothing was sent, so
    // nothing happened. Reporting it as ambiguous would send a reader investigating an effect that never
    // existed — the same choice `jarvis-tools`' filesystem adapter makes before its first read.
    use jarvis_tools::ToolExecutor;

    let transport = Scripted {
        result: Ok(response(200, r#"{"messages":[]}"#)),
    };
    let token = must(
        AccessToken::new("ya29.a0AfH6SMBsecretvalue1234567890"),
        "valid",
    );
    let adapter = GoogleReadTool::new(&transport, &token, "google-read");
    // A deadline in the Unix epoch, which every real clock is past.
    let request = authorized(
        "google.gmail_messages_list",
        json!({ "query": "is:unread" }),
        UtcTimestamp::from_unix_nanos(1).unwrap_or_else(|_| panic!("a representable instant")),
    );
    let error = adapter
        .execute(&request)
        .await
        .err()
        .unwrap_or_else(|| panic!("a lapsed deadline must be refused"));
    assert!(
        matches!(error, AdapterError::RefusedBeforeReaching { .. }),
        "a call that was never sent is a certain refusal, not an ambiguity: {error:?}"
    );
    assert!(error.to_string().contains("deadline"), "{error}");
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
    // Every declared operation is a read, so the port's method set has one member. `P5-009` must extend
    // both together, which is what makes a write a deliberate edit rather than a convenience.
    let request = must(
        request_for("google.gmail_messages_list", &json!({})),
        "a read request",
    );
    assert_eq!(must(method_for(&request), "a GET"), HttpMethod::Get);
    assert_eq!(HttpMethod::Get.as_str(), "GET");
    assert_eq!(HttpMethod::Get.to_string(), "GET");
}
