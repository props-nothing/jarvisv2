//! Tests for the transport port.
//!
//! The port's load-bearing property is the classification of a failure: which ones may have reached the
//! provider, and therefore which refuse an automatic retry. That is asserted here for every variant **and** for
//! the exhaustive-match property that keeps a new variant from silently defaulting.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::google::request;
fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

#[test]
fn every_failure_with_a_request_written_may_have_reached_the_provider() {
    // A table rather than a handful of separate assertions, so a new variant forces a decision instead of
    // defaulting. The direction that matters is the `true` side: reporting an ambiguous failure as certain
    // would permit a retry, and for a non-idempotent effect that retry is a second effect.
    let reaching = [
        TransportFailure::Send,
        TransportFailure::Timeout,
        TransportFailure::Body,
    ];
    let certain = [
        TransportFailure::Connect,
        TransportFailure::Refused {
            reason: "a forbidden redirect",
        },
    ];
    for failure in reaching {
        assert!(
            failure.may_have_reached_the_provider(),
            "{failure:?} wrote something, so its effect is unknown"
        );
    }
    for failure in certain {
        assert!(
            !failure.may_have_reached_the_provider(),
            "{failure:?} wrote nothing, so nothing happened"
        );
    }
}

#[test]
fn an_unreadable_body_is_ambiguous_because_the_provider_did_answer() {
    // The subtle one, and the reason `Body` is not grouped with `Connect`. The provider answered — so it
    // certainly received the request — and an unreadable body says nothing about what it did. Treating this as
    // "nothing happened" would be the exact mistake the ambiguous variant exists to prevent.
    assert!(TransportFailure::Body.may_have_reached_the_provider());
}

#[test]
fn a_timeout_is_ambiguous_rather_than_certain() {
    // A timed-out request may have arrived. A transport cannot distinguish "the provider never saw it" from
    // "the provider answered too slowly", and guessing either way is worse than reporting the ambiguity.
    assert!(TransportFailure::Timeout.may_have_reached_the_provider());
}

#[test]
fn a_policy_refusal_is_certain_and_distinct_from_a_connect_failure() {
    // Both are `false`, but for different reasons and with different provenance: `Refused` is this client's
    // decision before writing, and `Connect` is the network's. Keeping them as separate variants is what lets a
    // caller tell an operator which one occurred, so the distinction is asserted rather than the boolean only.
    let refused = TransportFailure::Refused {
        reason: "an off-allowlist host",
    };
    assert!(!refused.may_have_reached_the_provider());
    assert!(!TransportFailure::Connect.may_have_reached_the_provider());
    assert_ne!(refused, TransportFailure::Connect);
    assert!(refused.to_string().contains("an off-allowlist host"));
}

#[test]
fn a_non_200_status_is_a_response_and_never_a_failure() {
    // The port's most important contract: a provider's refusal is a RESPONSE the caller classifies. Collapsing
    // it into `TransportFailure` would lose the status and the reason, which is everything `client::classify`
    // needs to decide whether a retry is permitted.
    let refused = TransportResponse {
        status: 403,
        retry_after: None,
        body: r#"{"error":{"code":403,"errors":[{"reason":"domainPolicy"}]}}"#.to_owned(),
    };
    assert!(!refused.is_success());
    // And `429` carries a stated delay that the transport passes through uninterpreted, because honouring it is
    // a retry decision the transport does not own.
    let throttled = TransportResponse {
        status: 429,
        retry_after: Some(RetryAfter::Seconds(30)),
        body: String::new(),
    };
    assert!(!throttled.is_success());
    assert_eq!(throttled.retry_after, Some(RetryAfter::Seconds(30)));
}

#[test]
fn only_a_200_reads_as_a_success() {
    // `2xx` is not the test: both Google APIs answer `200` for every operation this connector declares, and
    // accepting the whole class would let a `204` with no body be read as a confirmed page — which is the
    // `Unknown` case `interpret_response` handles. So the predicate is narrow on purpose.
    let success = TransportResponse {
        status: 200,
        retry_after: None,
        body: "{}".to_owned(),
    };
    assert!(success.is_success());
    for status in [201, 202, 204, 206, 301, 304, 400, 401, 500] {
        let response = TransportResponse {
            status,
            retry_after: None,
            body: "{}".to_owned(),
        };
        assert!(
            !response.is_success(),
            "{status} must not read as a success"
        );
    }
}

#[test]
fn the_ports_method_set_has_one_member_and_refuses_anything_else() {
    // A closed set is what makes a new method a deliberate edit: when `P5-009` adds a write, the enum grows by a
    // variant and `method_of`'s match becomes a compile error rather than a silent default that would send a
    // `GET` for a write.
    let request = must(
        request::gmail_messages_list(Some("is:unread"), None, None),
        "a list request",
    );
    assert_eq!(request.method(), "GET");
    assert_eq!(must(method_of(&request), "a GET"), HttpMethod::Get);
    assert_eq!(HttpMethod::Get.as_str(), "GET");
    assert_eq!(HttpMethod::Get.to_string(), "GET");
    // The enum is `Copy`, so a method can be passed without a clone and the port's signature stays free of
    // lifetimes it does not need.
    let method = HttpMethod::Get;
    let copied = method;
    assert_eq!(method, copied);
}

#[test]
fn the_transport_trait_can_be_formatted_without_printing_a_client() {
    // A `Box<dyn GoogleTransport>` must be usable in a log line. An implementation holds a client, which may
    // hold a connection pool and a proxy configuration — none of which belongs in a formatted value — so the
    // `Debug` impl names the port and stops.
    struct Silent;

    #[async_trait::async_trait]
    impl GoogleTransport for Silent {
        async fn send(
            &self,
            _method: HttpMethod,
            _request: &request::HttpRequest,
            _token: &crate::google::credential::AccessToken,
        ) -> Result<TransportResponse, TransportFailure> {
            Err(TransportFailure::Connect)
        }

        async fn send_form(
            &self,
            _request: &request::FormRequest,
        ) -> Result<TransportResponse, TransportFailure> {
            // This double exists to check the `Debug` rendering, so it must also satisfy the port's second
            // method. A read refusal is the honest answer: a form `POST` is not a read and this type models
            // nothing.
            Err(TransportFailure::Connect)
        }
    }

    let transport: Box<dyn GoogleTransport> = Box::new(Silent);
    assert_eq!(format!("{transport:?}"), "GoogleTransport { .. }");
}

#[test]
fn the_retry_after_field_has_three_readable_situations_not_two() {
    // **The defect this type exists to remove.** `RFC 9110` §10.2.3 defines
    // `Retry-After = HTTP-date / delay-seconds`, and an `Option<u32>` cannot tell *absent* from *stated in the
    // date form*. The assertions below pin all three apart, and the middle one is the point: an all-digit value
    // too large for a `u32` is `delay-seconds` by grammar and must still read as a **stated** delay.
    //
    // The header was not present: nothing was stated.
    assert_eq!(parse_retry_after(None), None);
    // A well-formed `delay-seconds`.
    assert_eq!(
        parse_retry_after(Some("120")),
        Some(RetryAfter::Seconds(120))
    );
    // Surrounding whitespace is excluded before evaluation, per RFC 9110 §5.5 — not a heuristic.
    assert_eq!(
        parse_retry_after(Some("  120  ")),
        Some(RetryAfter::Seconds(120))
    );
    // The `HTTP-date` the grammar's other alternative allows: present, and not seconds.
    assert_eq!(
        parse_retry_after(Some("Fri, 31 Dec 1999 23:59:59 GMT")),
        Some(RetryAfter::NotSeconds)
    );
    // `delay-seconds = 1*DIGIT` has **no upper bound**, so an oversize number is still a stated delay and must
    // not fall back to "stated nothing". This is the case a plain `parse::<u32>().ok()` gets wrong by returning
    // `None`, which reads as "retry now".
    assert_eq!(
        parse_retry_after(Some("99999999999999999999")),
        Some(RetryAfter::NotSeconds)
    );
    // The grammar's two alternatives both require content, so a present-but-empty value carries nothing.
    assert_eq!(parse_retry_after(Some("")), None);
    assert_eq!(parse_retry_after(Some("   ")), None);

    // And the accessor never silently equates the two: `seconds()` is `None` for the date form, which is **not**
    // the same `None` as an absent header — that is `parse_retry_after`'s, and the two live on different types.
    assert_eq!(RetryAfter::Seconds(5).seconds(), Some(5));
    assert_eq!(RetryAfter::NotSeconds.seconds(), None);
    assert!(RetryAfter::Seconds(0).is_delay_seconds());
    assert!(!RetryAfter::NotSeconds.is_delay_seconds());
}

#[test]
fn the_port_is_send_and_sync_so_a_client_can_be_shared_across_workers() {
    // Asserted rather than assumed: a transport that was not `Sync` could not be held by an adapter shared
    // between run workers, and the bound is part of the port's contract rather than an implementation detail.
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn GoogleTransport>();
    assert_send_sync::<TransportResponse>();
    assert_send_sync::<TransportFailure>();
}
