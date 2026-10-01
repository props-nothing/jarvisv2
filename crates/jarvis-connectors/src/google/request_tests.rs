//! Tests for Google request construction.
//!
//! The encoding tests are the security-relevant ones: a query is model-chosen text, so a value that changes
//! the request's *structure* rather than its content is an injection. Each assertion names the character that
//! would do it.
//!
//! Falsification record in `TODO.md`.

use super::*;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

#[test]
fn percent_encoding_leaves_the_unreserved_set_alone() {
    // RFC 3986's unreserved set, asserted character by character. A permissive encoder that let, say, `+`
    // through would be indistinguishable from a correct one on a plain `is:unread` query, and the difference
    // only appears on a value that needs it.
    for character in "abcXYZ019-._~".chars() {
        assert_eq!(
            percent_encode(&character.to_string()),
            character.to_string(),
            "`{character}` is unreserved and must not be encoded"
        );
    }
    assert_eq!(percent_encode("is:unread"), "is%3Aunread");
}

#[test]
fn a_query_value_cannot_change_the_requests_structure() {
    // The characters that would alter the request rather than its content, each asserted as its **exact**
    // encoding because each is a different attack:
    //  - `&` starts a NEW parameter, so `is:unread&maxResults=999` would replace the bound with the model's.
    //  - `=` would make the provider read the text as a parameter assignment.
    //  - `#` ends the query entirely, so everything after it is dropped.
    //  - `%` would start an escape sequence of the sender's choosing.
    //  - `?` would start a *second* query string.
    //
    // Pinned as literals rather than as a "the character must not appear" property, and the reason is `%`: its
    // escape is `%25`, which **contains** `%` as the escape marker, so the property is false for the one
    // character that most obviously needs escaping. An earlier version of this test asserted the property and
    // failed on `%` — the test was wrong and the encoder was right.
    for (hazardous, expected) in [
        ('&', "%26"),
        ('=', "%3D"),
        ('#', "%23"),
        ('%', "%25"),
        ('?', "%3F"),
    ] {
        let encoded = percent_encode(&hazardous.to_string());
        assert_eq!(
            encoded, expected,
            "`{hazardous}` must encode to `{expected}`, not `{encoded}`"
        );
        // And the character appears only as the escape marker, never as itself: the first byte is `%` and the
        // remainder is hex, so no raw occurrence survives.
        assert!(encoded.starts_with('%'));
        assert!(
            !encoded[1..].contains(hazardous),
            "`{hazardous}` must appear only as the escape marker"
        );
    }
    // And the concrete shape: the parameter count is what a reader can check.
    let request = must(
        gmail_messages_list(Some("is:unread&maxResults=999"), None, None),
        "a query with an ampersand must still build one request",
    );
    assert_eq!(
        request.query().len(),
        1,
        "the value must not become a parameter"
    );
    assert_eq!(request.query()[0].0, "q");
    assert!(
        request.query()[0].1.contains("%26"),
        "the ampersand must be escaped"
    );
    assert!(
        !request.url_with_query().contains("&maxResults="),
        "the injection must not appear as a parameter: {}",
        request.url_with_query()
    );
}

#[test]
fn a_space_and_a_literal_plus_are_encoded_so_neither_is_ambiguous() {
    // `application/x-www-form-urlencoded` writes a space as `+`, and Gmail's search syntax uses a literal `+`
    // (`larger:10M` has no `+`, but `{+label}` is not needed either — the point is that a receiver which
    // decoded `+` as a space would misread a value containing one). Encoding both removes the ambiguity.
    assert_eq!(
        percent_encode("in:inbox larger:10M"),
        "in%3Ainbox%20larger%3A10M"
    );
    assert_eq!(percent_encode("a+b"), "a%2Bb");
    // The two must be distinguishable after decoding, which is the property that matters.
    assert_ne!(percent_encode("a b"), percent_encode("a+b"));
}

#[test]
fn hex_escapes_are_uppercase_as_normalisation_requires() {
    // RFC 3986 §6.2.2.1: lowercase hex is equivalent but not canonical. A recorded fixture that used
    // lowercase would differ from what the provider echoes, so the case is pinned rather than left to chance.
    assert_eq!(percent_encode("\u{7f}"), "%7F");
    assert_eq!(percent_encode("é"), "%C3%A9");
    assert!(
        !percent_encode("é").contains("%c3"),
        "lowercase hex is not the canonical form"
    );
}

#[test]
fn a_multibyte_character_is_encoded_per_utf8_byte() {
    // The encoder works on BYTES, so a multi-byte character becomes several escapes. Encoding per `char` via
    // `char::to_string` and then per byte would give the same answer here, but an encoder that tried to escape
    // the code point as one number would produce a different — and wrong — value.
    assert_eq!(percent_encode("é"), "%C3%A9");
    assert_eq!(percent_encode("日"), "%E6%97%A5");
}

#[test]
fn the_request_carries_no_credential_in_its_url() {
    // The documented alternative is `?access_token=…`, and Google's own page says "query strings tend to be
    // visible in server logs". A type that cannot hold a token is stronger than a rule: this asserts the
    // absence on every request the module can build.
    let requests = [
        must(
            gmail_messages_list(Some("is:unread"), Some(10), None),
            "a list request",
        ),
        must(
            gmail_messages_get("abc123", MessageFormat::Minimal),
            "a get request",
        ),
        must(
            calendar_events_list("primary", None, None, None, None, None),
            "an events request",
        ),
    ];
    for request in &requests {
        let url = request.url_with_query();
        for forbidden in ["access_token", "token=", "key=", "api_key"] {
            assert!(
                !url.contains(forbidden),
                "{} must not carry `{forbidden}` in its URL",
                request.url()
            );
        }
        // The parameter names are this module's own constants, so a credential could only appear as a VALUE.
        // Asserted by name, because a name is what the module controls.
        for (name, _) in request.query() {
            assert!(
                !name.contains("token") || name == "pageToken" || name == "syncToken",
                "`{name}` would be a credential position"
            );
        }
    }
    // The header names ARE stated, so a transport knows where a credential goes — and the token is not here.
    assert_eq!(AUTHORIZATION_HEADER, "Authorization");
    assert_eq!(BEARER_SCHEME, "Bearer");
}

#[test]
fn a_page_token_is_validated_on_the_way_out_as_well_as_in() {
    // A token from a *response* becomes the next request's parameter, so the one bound has to apply in both
    // directions. An empty token is a different statement from an absent one: absent means the last page.
    assert_eq!(
        must(gmail_messages_list(None, None, None), "a request")
            .query()
            .len(),
        0,
        "an absent page token adds no parameter"
    );
    let with_token = must(
        gmail_messages_list(None, None, Some("CAEQAA")),
        "a request with a page token",
    );
    assert_eq!(with_token.query().len(), 1);
    assert_eq!(with_token.query()[0].0, "pageToken");
    assert!(next_page_is_refused(Some("")));
    assert!(next_page_is_refused(Some("a\nb")));
}

/// Returns whether `client::next_page` refuses a token, so the bound is asserted without duplicating it.
fn next_page_is_refused(token: Option<&str>) -> bool {
    client::next_page(token).is_err()
}

#[test]
fn a_page_token_and_a_sync_token_cannot_arrive_together_and_the_pair_is_reported_rather_than_resolved()
 {
    // **This test used to assert the opposite, and that was the defect `ADR-0109` records.** It fed a body
    // carrying BOTH tokens and asserted the parser yielded both — a body Google documents as impossible, since
    // `nextPageToken` is *"Omitted if no further results are available, in which case `nextSyncToken` is
    // provided"* and `nextSyncToken` is *"Omitted if further results are available, in which case `nextPageToken`
    // is provided"*. The constraint had already been recorded in the fixture's own notes and in the renderer's
    // test helper; the parser's test was the layer that never heard about it.
    //
    // So the body is still exercised — deliberately, as the **nonconforming** case — and the assertion is now
    // that it is *reported* rather than silently resolved into a token. A parser that preferred one would hand a
    // caller a sync position for a walk that has not finished.
    let body = r#"{
        "items": [{"id": "event-1"}, {"id": "event-2"}],
        "nextPageToken": "page-token-1",
        "nextSyncToken": "sync-token-1"
    }"#;
    let page = must(
        parse_calendar_page(200, body),
        "a body with both tokens must still parse into a reportable state",
    );
    assert_eq!(page.ids, ["event-1", "event-2"]);
    assert_eq!(
        page.continuation,
        client::CalendarContinuation::Rejected {
            page_token: "page-token-1".to_owned(),
            sync_token: "sync-token-1".to_owned(),
        },
        "a pair the provider documents as impossible must be named, not resolved"
    );
    assert!(page.continuation.is_nonconforming());
    // And neither half is offered as usable, which is the property that matters: a caller cannot store a sync
    // position, and cannot fetch a next page, from a response nobody can interpret.
    assert_eq!(page.continuation.storable(), None);
    assert_eq!(page.continuation.page_token(), None);

    // The two conforming shapes, asserted apart — which is the pair the fixtures carry.
    let mid_walk = must(
        parse_calendar_page(
            200,
            r#"{"items":[{"id":"e1"}],"nextPageToken":"page-token-1"}"#,
        ),
        "a mid-walk page",
    );
    assert_eq!(
        mid_walk.continuation,
        client::CalendarContinuation::MorePages {
            page_token: "page-token-1".to_owned()
        }
    );
    assert_eq!(
        mid_walk.continuation.storable(),
        None,
        "a page token is not a position, so a mid-walk page yields nothing to store"
    );
    assert_eq!(mid_walk.continuation.page_token(), Some("page-token-1"));

    let last = must(
        parse_calendar_page(
            200,
            r#"{"items":[{"id":"e3"}],"nextSyncToken":"sync-token-1"}"#,
        ),
        "a last page",
    );
    assert_eq!(
        last.continuation,
        client::CalendarContinuation::WalkComplete {
            sync_token: "sync-token-1".to_owned()
        }
    );
    assert_eq!(last.continuation.storable(), Some("sync-token-1"));
    assert_eq!(
        last.continuation.page_token(),
        None,
        "a completed walk has no next page to fetch"
    );

    // And neither token at all is its own state rather than an empty token or a page token.
    let empty = must(
        parse_calendar_page(200, r#"{"items":[]}"#),
        "a page with no continuation",
    );
    assert_eq!(
        empty.continuation,
        client::CalendarContinuation::NothingFurther
    );
    assert_eq!(empty.continuation.storable(), None);
}

#[test]
fn a_sync_token_absent_from_a_middle_page_is_not_an_empty_one() {
    // The middle pages of a paginated incremental sync carry no sync token at all — and, because the two tokens
    // are mutually exclusive, that absence is exactly what the page token beside it means. Reading it as an
    // empty token would store a cursor that addresses nothing.
    let body = r#"{"items": [{"id": "event-1"}], "nextPageToken": "more"}"#;
    let page = must(parse_calendar_page(200, body), "a valid events page");
    assert_eq!(
        page.continuation,
        client::CalendarContinuation::MorePages {
            page_token: "more".to_owned()
        },
        "a page token implies no sync token, and one value says both"
    );
    assert_eq!(page.continuation.storable(), None);
}

#[test]
fn an_error_status_is_never_parsed_as_an_empty_page() {
    // The failure mode this guards: an error document parsed as a page reports "no results", and a caller
    // cannot tell a successful empty mailbox from a refused request. So the status is checked first and every
    // non-200 is refused with the reason naming `client::classify` as where the outcome belongs.
    let error_body = r#"{"error":{"code":403,"errors":[{"reason":"domainPolicy"}]}}"#;
    for status in [400, 401, 403, 404, 429, 500] {
        assert!(
            parse_id_page(status, error_body).is_err(),
            "status {status} must not be parsed as a page"
        );
        assert!(parse_calendar_page(status, error_body).is_err());
    }
    // The positive control: a genuine empty page IS a page, so the refusal above is about the status rather
    // than about a body with no results.
    let empty = must(
        parse_id_page(200, r#"{"resultSizeEstimate": 0}"#),
        "an empty page is a page",
    );
    assert!(empty.ids.is_empty());
    assert_eq!(empty.next_page_token, None);
}

#[test]
fn a_body_of_the_wrong_shape_is_refused_rather_than_silently_empty() {
    // `messages` and `items` both default to empty, so a body that carries neither parses successfully — which
    // is correct for `{"resultSizeEstimate": 0}` and would be wrong to accept as a *different* document. What
    // is refused is a body that is not JSON at all, and a resource with no `id`.
    assert!(parse_id_page(200, "not json").is_err());
    assert!(parse_single_message(200, "not json").is_err());
    assert!(parse_single_message(200, r#"{"threadId": "t"}"#).is_err());
    let message = must(
        parse_single_message(200, r#"{"id": "msg-1", "threadId": "t"}"#),
        "a valid resource",
    );
    assert_eq!(message.id, "msg-1");
    assert_eq!(message.thread_id.as_deref(), Some("t"));
}

#[test]
fn an_absent_label_list_is_not_an_empty_one() {
    // The distinction that a `Vec` with `#[serde(default)]` would erase. `minimal` and `metadata` returns carry
    // `labelIds`; a body without the field means the provider did not return it, which is a different fact from
    // "the message has no labels". Only the second renders an empty array, and the renderer omits the key for
    // the first — so `None` and `Some(vec![])` must stay distinguishable here or the output lies about one of
    // them (`ADR-0083`).
    let absent = must(
        parse_single_message(200, r#"{"id": "m"}"#),
        "a body with no labels field",
    );
    assert_eq!(absent.label_ids, None);

    let empty = must(
        parse_single_message(200, r#"{"id": "m", "labelIds": []}"#),
        "a body with an empty labels array",
    );
    assert_eq!(empty.label_ids, Some(Vec::new()));

    assert_ne!(
        absent, empty,
        "an absent label list must not equal an empty one"
    );
}

#[test]
fn max_results_is_bounded_per_api_and_zero_is_refused() {
    // Zero refused rather than read as "unlimited": a zero read as unlimited is an unbounded request, and here
    // Google would answer with a default the caller did not choose. The two APIs have different caps, so a
    // single shared bound would be wrong for one of them.
    assert!(gmail_messages_list(None, Some(0), None).is_err());
    assert!(gmail_messages_list(None, Some(GMAIL_MAX_RESULTS_CAP + 1), None).is_err());
    assert!(gmail_messages_list(None, Some(GMAIL_MAX_RESULTS_CAP), None).is_ok());
    assert!(
        calendar_events_list(
            "primary",
            None,
            None,
            Some(GMAIL_MAX_RESULTS_CAP + 1),
            None,
            None
        )
        .is_ok()
    );
    assert!(
        calendar_events_list(
            "primary",
            None,
            None,
            Some(CALENDAR_MAX_RESULTS_CAP + 1),
            None,
            None
        )
        .is_err()
    );
    let calendar = must(
        calendar_events_list("primary", None, None, Some(1_000), None, None),
        "a Calendar page of 1000 is within its own cap",
    );
    assert_eq!(calendar.query()[0].1, "1000");
}

#[test]
fn a_time_range_cannot_be_sent_with_a_sync_token() {
    // **A request the provider documents as impossible, refused before it is built.** `events.list` lists
    // `timeMin` and `timeMax` among the parameters that "cannot be specified together with nextSyncToken to
    // ensure consistency of the client state", so sending both is a `400` by construction. The pairing is
    // refused locally because it can *never* succeed: a refusal from the provider would spend a request to
    // learn something already documented and would still not tell the caller which argument to drop.
    for (time_min, time_max) in [
        (Some("2026-01-01T00:00:00Z"), None),
        (None, Some("2026-12-31T00:00:00Z")),
        (Some("2026-01-01T00:00:00Z"), Some("2026-12-31T00:00:00Z")),
    ] {
        let refused =
            calendar_events_list("primary", time_min, time_max, None, None, Some("sync-1"))
                .err()
                .unwrap_or_else(|| {
                    panic!(
                        "a time range with a sync token must be refused: {time_min:?} {time_max:?}"
                    )
                });
        assert!(
            matches!(refused, RequestError::DisallowedCombination { .. }),
            "the refusal must name the combination, not one value: {refused:?}"
        );
    }

    // **Each argument alone is still accepted**, so the refusal is about the combination and not about either
    // value. Without these two the test would pass for a builder that refused every `time_min` or every
    // `sync_token`, which would break the full sync and the incremental sync respectively.
    assert!(
        calendar_events_list(
            "primary",
            Some("2026-01-01T00:00:00Z"),
            None,
            None,
            None,
            None
        )
        .is_ok()
    );
    assert!(calendar_events_list("primary", None, None, None, None, Some("sync-1")).is_ok());

    // And a full sync with both bounds, which the sync guide's own sample uses ("we are only syncing events up
    // to a year old`"), so the restriction is specifically the token and not the range.
    assert!(
        calendar_events_list(
            "primary",
            Some("2026-01-01T00:00:00Z"),
            Some("2026-12-31T00:00:00Z"),
            None,
            None,
            None
        )
        .is_ok()
    );
}

#[test]
fn a_refusal_names_the_argument_the_caller_actually_sent() {
    // **The defect: a shared validator hard-coded the field name it reported.** `search_query` said
    // `field: "query"` while it also validated Calendar's `time_min` and `time_max`, so an oversized time bound
    // was refused with *"the `query` argument is unusable"* — naming an argument the caller never supplied, and
    // calling an RFC 3339 instant "a search query". A caller reading that would go looking for a `query`
    // parameter that does not exist on `calendar_events_read` (`ADR-0086`).
    for (label, refused) in [
        (
            "time_min",
            calendar_events_list(
                "primary",
                Some(&"2".repeat(MAX_TIME_BOUND_CHARS + 1)),
                None,
                None,
                None,
                None,
            ),
        ),
        (
            "time_max",
            calendar_events_list(
                "primary",
                None,
                Some(&"2".repeat(MAX_TIME_BOUND_CHARS + 1)),
                None,
                None,
                None,
            ),
        ),
    ] {
        let RequestError::Argument { field, reason } = refused.err().unwrap_or_else(|| {
            panic!("an oversized {label} must be refused");
        }) else {
            panic!("an oversized {label} must be an argument fault");
        };
        assert_eq!(
            field, label,
            "the refusal must name the argument the caller sent, not `query`"
        );
        assert!(
            !reason.contains("search query"),
            "an RFC 3339 instant is not a search query: {reason}"
        );
    }

    // The control: the Gmail list's own `query` argument still reports `query`, so threading the field through
    // did not just rename every refusal. Without this a validator hard-coded to `time_min` would pass above.
    let RequestError::Argument { field, .. } =
        gmail_messages_list(Some(&"a".repeat(MAX_QUERY_CHARS + 1)), None, None)
            .err()
            .unwrap_or_else(|| panic!("an oversized query must be refused"))
    else {
        panic!("an oversized query must be an argument fault");
    };
    assert_eq!(field, "query");
}

#[test]
fn a_time_bound_is_validated_as_a_bound_and_not_as_a_search_query() {
    // The two bounds are different kinds of value, so the accepted length is not the query's 512. An instant is
    // about 25 characters; the bound admits a generous one and refuses a string that is plainly not a
    // timestamp, which is the check that stops a model-supplied string becoming an unbounded URL value.
    //
    // A `const` assertion because both sides are constants: a runtime assertion would be optimised to nothing,
    // and clippy rejects `assert!` on a constant expression for exactly that reason.
    const _: () = assert!(MAX_TIME_BOUND_CHARS < MAX_QUERY_CHARS);
    assert!(
        calendar_events_list(
            "primary",
            Some(&"2".repeat(MAX_TIME_BOUND_CHARS)),
            None,
            None,
            None,
            None,
        )
        .is_ok(),
        "a bound at the limit must be accepted, or the limit is unreachable"
    );
    // A real instant with an offset and fractional seconds is well inside it, so the bound does not refuse a
    // value the provider would accept.
    assert!(
        calendar_events_list(
            "primary",
            Some("2026-09-27T00:00:00.123456+02:00"),
            None,
            None,
            None,
            None,
        )
        .is_ok()
    );
    // A control character is refused for the same reason every value here refuses one: the bound becomes a URL
    // query value and a log field, and a newline in a log field forges a record.
    let control = calendar_events_list(
        "primary",
        Some("2026-01-01T00:00:00Z\nx"),
        None,
        None,
        None,
        None,
    )
    .err()
    .unwrap_or_else(|| panic!("a control character must be refused"));
    assert!(
        matches!(
            control,
            RequestError::Argument {
                field: "time_min",
                ..
            }
        ),
        "the refusal must name `time_min`: {control:?}"
    );
}

#[test]
fn a_resource_identifier_is_validated_and_encoded_as_a_path_segment() {
    // The identifier goes in the PATH, so a `/` in it would change which resource is addressed — a different
    // reason for the same encoding a query value needs.
    assert!(gmail_messages_get("", MessageFormat::Full).is_err());
    assert!(gmail_messages_get("  ", MessageFormat::Full).is_err());
    assert!(gmail_messages_get("a\nb", MessageFormat::Full).is_err());
    assert!(
        gmail_messages_get(&"a".repeat(MAX_RESOURCE_ID_CHARS + 1), MessageFormat::Full).is_err()
    );
    // At the bound is accepted, so the bound is reachable rather than an unreachable one.
    assert!(gmail_messages_get(&"a".repeat(MAX_RESOURCE_ID_CHARS), MessageFormat::Full).is_ok());

    let hazardous = must(
        gmail_messages_get("msg/../other", MessageFormat::Full),
        "an identifier with a slash builds a request",
    );
    assert!(
        !hazardous.url().contains("msg/../other"),
        "a slash in an identifier must not survive into the path: {}",
        hazardous.url()
    );
    assert!(hazardous.url().contains("%2F"), "the slash must be escaped");
}

#[test]
fn the_message_format_cannot_express_raw() {
    // The type is the control rather than a check: `format=raw` returns the unparsed MIME message, nothing in
    // this connector parses it, and a caller cannot ask for it because no variant names it.
    assert_eq!(MessageFormat::Minimal.as_str(), "minimal");
    assert_eq!(MessageFormat::Metadata.as_str(), "metadata");
    assert_eq!(MessageFormat::Full.as_str(), "full");
    let formats = [
        MessageFormat::Minimal,
        MessageFormat::Metadata,
        MessageFormat::Full,
    ];
    for format in formats {
        assert_ne!(format.as_str(), "raw");
    }
}

#[test]
fn a_parameter_is_omitted_rather_than_sent_empty() {
    // `name=` is a different request from omitting the parameter, and for `pageToken` the difference is
    // between "the last page" and "a token Google rejects".
    let bare = must(
        gmail_messages_list(None, None, None),
        "a request with no arguments",
    );
    assert!(bare.query().is_empty());
    assert_eq!(
        bare.url_with_query(),
        bare.url(),
        "no query string when nothing is set"
    );
    // And an empty query TEXT is a legitimate request rather than an omitted one, so it IS sent.
    let empty_query = must(
        gmail_messages_list(Some(""), None, None),
        "an empty search query",
    );
    assert_eq!(empty_query.query().len(), 1);
    assert_eq!(empty_query.query()[0].1, "");
}

#[test]
fn the_rendering_reports_parameter_names_and_never_a_value() {
    // A rendering reaches a log line, and a Value is model-chosen text. So `Display` reports the method, the
    // path, and the parameter NAMES — a name is this module's own constant, a value is not.
    let request = must(
        gmail_messages_list(Some("from:alice@example.com SECRET"), Some(5), None),
        "a valid request",
    );
    let rendered = request.to_string();
    assert!(
        !rendered.contains("SECRET"),
        "a value must not be rendered: {rendered}"
    );
    assert!(
        !rendered.contains("alice"),
        "a value must not be rendered: {rendered}"
    );
    assert!(
        rendered.contains('q'),
        "a parameter name is this module's own constant"
    );
    assert!(rendered.contains("maxResults"));
    // And the URL alone carries no query, which is the split that makes the rendering possible.
    assert!(!request.url().contains('?'));
    assert!(request.url_with_query().contains('?'));
}

#[test]
fn the_urls_are_the_documented_api_bases_and_paths() {
    // A URL is the one part of a request a reader can compare against the research record, so the shape is
    // pinned: `/users/me/messages` for Gmail and `/calendars/{id}/events` for Calendar, on the documented
    // bases. `me` is the self-referential segment both APIs document.
    let list = must(gmail_messages_list(None, None, None), "a request");
    assert_eq!(
        list.url(),
        "https://www.googleapis.com/gmail/v1/users/me/messages"
    );
    assert_eq!(list.method(), "GET");
    assert_eq!(list.accept(), JSON_ACCEPT);

    let get = must(gmail_messages_get("abc", MessageFormat::Full), "a request");
    assert_eq!(
        get.url(),
        "https://www.googleapis.com/gmail/v1/users/me/messages/abc"
    );

    let events = must(
        calendar_events_list("primary", None, None, None, None, None),
        "a request",
    );
    assert_eq!(
        events.url(),
        "https://www.googleapis.com/calendar/v3/calendars/primary/events"
    );

    // And the helper that classifies a URL for a diagnostic, which must return `None` for anything unknown
    // rather than defaulting to one API.
    assert_eq!(api_host_of(list.url()), Some("gmail"));
    assert_eq!(api_host_of(events.url()), Some("calendar"));
    assert_eq!(api_host_of("https://example.invalid/x"), None);
    assert_eq!(connector_id(), "google");
}

#[test]
fn a_history_request_names_the_starting_position_and_is_bounded() {
    // The incremental-sync read. The starting position is REQUIRED, because the method reference says so:
    // "startHistoryId — Required. Returns history records after the specified startHistoryId."
    let request = must(
        gmail_history_list("12345", Some(50), None),
        "a history request",
    );
    assert_eq!(
        request.url(),
        "https://www.googleapis.com/gmail/v1/users/me/history"
    );
    assert_eq!(request.method(), "GET");
    assert_eq!(
        request.query()[0],
        ("startHistoryId".to_owned(), "12345".to_owned())
    );
    // The credential boundary holds on this path too: no field of the request can hold a token, and the
    // rendered URL proves a caller cannot have put one there.
    assert!(
        !request.url_with_query().contains("access_token"),
        "a token must not be representable in a URL"
    );
    // An empty position addresses nothing, and an oversized or control-bearing one becomes a different
    // request, so all three are refused rather than sent.
    assert!(gmail_history_list("", None, None).is_err());
    assert!(gmail_history_list("   ", None, None).is_err());
    assert!(gmail_history_list("a\nb", None, None).is_err());
    // The cap is the API's own 500, exercised AT its limit so the bound is not unreachable.
    assert!(gmail_history_list("1", Some(GMAIL_MAX_RESULTS_CAP), None).is_ok());
    assert!(gmail_history_list("1", Some(GMAIL_MAX_RESULTS_CAP + 1), None).is_err());
    assert!(gmail_history_list("1", Some(0), None).is_err());
}

#[test]
fn a_history_page_keeps_the_cursor_and_the_page_token_apart() {
    // The fixture carries BOTH `nextPageToken` and `historyId`, which is the shape the reference documents: the
    // page token is "the token for the NEXT PAGE of results", while the history id is "the ID of the mailbox's
    // current history record". They are not a mutually-exclusive pair like Calendar's two tokens, so a reader
    // that treated them as one would take its cursor from whichever field it happened to read.
    let text = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("google")
            .join("gmail_history_list.json"),
    )
    .unwrap_or_else(|_| panic!("the history fixture must be readable"));
    let page = must(parse_history_page(200, &text), "the documented shape");
    assert_eq!(page.ids.len(), 2);
    // The mid-walk fixture carries a page token **and** a history id, so the id is stated but **not storable**
    // -- the reference ties storing to the absence of the token, and a walk with more to come has not consumed
    // the changes up to this position.
    assert_eq!(page.next_page_token.as_deref(), Some("0987654321"));
    assert_eq!(
        page.position,
        client::HistoryPosition::UnfinishedWalk {
            stated_history_id: "12347".to_owned()
        }
    );
    assert_eq!(page.position.storable(), None);
    assert!(
        !page.position.is_storable(),
        "a page with a token beside it states a position the provider never said to store"
    );
    // Both history records contributed their messages, so the change set is flat rather than one id per record.
    assert_eq!(page.ids.len(), 2);
    assert_eq!(page.ids[0], "18c1f2a3b4d5e6f7");

    // A status other than 200 is refused rather than read as an empty change set -- an error document parsed
    // as a page would report "no changes" and a caller could not tell that from a quiet mailbox.
    assert!(parse_history_page(404, &text).is_err());
    assert!(parse_history_page(500, &text).is_err());
    assert!(parse_history_page(200, "not json").is_err());
}

#[test]
fn a_time_bound_is_encoded_like_any_other_value() {
    // An RFC 3339 instant contains `:` and `+`, both of which would change the request if sent raw — `+` in
    // particular, because a receiver applying the form-urlencoded rule would read it as a space and the
    // instant would be shifted.
    let request = must(
        calendar_events_list(
            "primary",
            Some("2026-09-27T00:00:00Z"),
            Some("2026-09-28T00:00:00+02:00"),
            None,
            None,
            None,
        ),
        "a valid range",
    );
    let url = request.url_with_query();
    assert!(url.contains("timeMin=2026-09-27T00%3A00%3A00Z"), "{url}");
    assert!(
        url.contains("%2B02%3A00"),
        "a literal `+` must be escaped: {url}"
    );
}

#[test]
fn a_page_token_and_a_sync_token_are_sent_together_to_walk_an_incremental_sync() {
    // **The shape the sync guide requires and this builder could not produce.** For a large incremental sync the
    // provider returns a `pageToken` *instead of* a sync token, and the guide says to repeat "the exact same list
    // query … with the exact same `syncToken`" and append the page token. So for the middle pages both are sent,
    // which is the opposite of the `timeMin`/`timeMax` restriction — the two rules are different and only one of
    // them is a conflict.
    let walk = must(
        calendar_events_list("primary", None, None, None, Some("page-2"), Some("sync-1")),
        "a sync token with a page token is the documented incremental walk",
    );
    let url = walk.url_with_query();
    assert!(url.contains("pageToken=page-2"), "{url}");
    assert!(
        url.contains("syncToken=sync-1"),
        "a page token must not displace the sync token: {url}"
    );

    // A page token alone is also legal — it continues a *full* walk, where there is no sync token yet. Without
    // this the test would pass for a builder that only sent the page token when a sync token was present.
    let first_walk = must(
        calendar_events_list("primary", None, None, None, Some("page-1"), None),
        "a page token alone continues a full walk",
    );
    assert!(
        first_walk.url_with_query().contains("pageToken=page-1"),
        "a page token with no sync token must still be sent"
    );

    // And an oversized token is refused by the same bound every token is, so the new parameter is not a hole in
    // the validation the other tokens go through.
    assert!(
        calendar_events_list(
            "primary",
            None,
            None,
            None,
            Some(&"a".repeat(crate::google::client::MAX_PAGE_TOKEN_CHARS + 1)),
            None
        )
        .is_err(),
        "a page token must go through the same length bound as every other token"
    );
}

#[test]
fn the_profile_request_addresses_the_calling_credential_and_takes_no_arguments() {
    // **The shape that makes naming another mailbox unrepresentable.** `users.getProfile`'s path parameter is
    // documented as "The user's email address. The special value `me` can be used to indicate the
    // authenticated user", and this builder always sends `me` — so the request cannot be aimed at an account
    // the caller's token does not authorise, which is why there is no `user_id` argument to validate.
    let request = gmail_profile();
    assert_eq!(request.method(), "GET");
    assert_eq!(
        request.url(),
        "https://www.googleapis.com/gmail/v1/users/me/profile"
    );
    // No query parameters: the request body "must be empty" and there is nothing to put in a query either, so
    // `url_with_query` is the path unchanged. Asserted because a stray parameter would be a fact about a
    // method that takes none.
    assert!(request.query().is_empty());
    assert_eq!(request.url_with_query(), request.url());
    assert_eq!(request.accept(), JSON_ACCEPT);
}

#[test]
fn a_profile_response_must_carry_an_address_because_that_is_the_whole_point() {
    // The reference gives the response as `{ "emailAddress": string, "messagesTotal": integer,
    // "threadsTotal": integer, "historyId": string }`. This connector reads two of the four — the identity and
    // the position — and the **address is required** because the operation exists to establish *which* mailbox
    // answered, so a profile without one establishes nothing.
    let profile = must(
        parse_profile(
            200,
            r#"{"emailAddress":"user@example.com","messagesTotal":42,"threadsTotal":7,"historyId":"1234567890"}"#,
        ),
        "the reference's profile must parse",
    );
    assert_eq!(profile.email_address, "user@example.com");
    assert_eq!(profile.history_id.as_deref(), Some("1234567890"));

    // An absent address is refused rather than returned as `None`, and a **whitespace-only** one is too: the
    // second satisfies "the field was present" while denoting nothing, which is the same mistake as an empty
    // value and the failure direction that would store an identity naming no account.
    for body in [
        r#"{"messagesTotal":1}"#,
        r#"{"emailAddress":null,"historyId":"1"}"#,
        r#"{"emailAddress":"   ","historyId":"1"}"#,
        r#"{"emailAddress":"","historyId":"1"}"#,
    ] {
        assert!(
            matches!(
                parse_profile(200, body),
                Err(RequestError::Argument {
                    field: "emailAddress",
                    ..
                })
            ),
            "`{body}` has no usable address and must be refused as the address rather than as a shape"
        );
    }
    // The position is optional, so a profile with only an identity is a valid result — the reference documents
    // `historyId` as its own field and a caller may want only to know which account this is.
    let identity_only = must(
        parse_profile(200, r#"{"emailAddress":"user@example.com"}"#),
        "an identity-only profile must parse",
    );
    assert_eq!(identity_only.history_id, None);
    // And a non-200 is refused **before** the body is read, so an error document cannot become an identity.
    assert!(parse_profile(403, r#"{"emailAddress":"user@example.com"}"#).is_err());
    // A body that is not a JSON object at all is refused as a body rather than as a missing address, so the
    // layer to check is named.
    assert!(matches!(
        parse_profile(200, "not json"),
        Err(RequestError::Argument { field: "body", .. })
    ));
}

#[test]
fn a_watch_body_carries_the_topic_and_only_the_filter_fields_that_govern_something() {
    // The `users.watch` reference: a JSON body of `topicName` plus optionally `labelIds` and
    // `labelFilterBehavior`. Asserted by **parsing the rendered body**, because the substance is which fields
    // are present — a string-containment check would pass on a body that also carried the deprecated spelling.
    let plain = must(
        gmail_watch("projects/p/topics/t", None, None),
        "a watch with no filter must build",
    );
    assert_eq!(
        plain.url(),
        "https://www.googleapis.com/gmail/v1/users/me/watch"
    );
    assert_eq!(plain.content_type(), "application/json");
    let body: serde_json::Value = must(
        serde_json::from_str(plain.rendered_body()),
        "the body is JSON",
    );
    assert_eq!(body["topicName"], "projects/p/topics/t");
    assert_eq!(
        body.get("labelIds"),
        None,
        "an unfiltered watch must not send a label list at all, rather than an empty one"
    );
    // **The deprecated field is absent, and this is the assertion that would fail if someone added it back.**
    // The reference says `labelFilterAction` is ignored when the new field is set and caused "incorrect
    // behavior in some cases" when it is not, so its presence would be a silent change of meaning.
    assert_eq!(
        body.get("labelFilterAction"),
        None,
        "the deprecated `labelFilterAction` must never be sent — `labelFilterBehavior` replaces it"
    );

    // With labels and a behaviour, both fields are present and the behaviour is the documented spelling.
    let scoped = must(
        gmail_watch(
            "projects/p/topics/t",
            Some(&["INBOX".to_owned()]),
            Some(crate::google::client::LabelFilterBehavior::Include),
        ),
        "a scoped watch must build",
    );
    let scoped_body: serde_json::Value = must(
        serde_json::from_str(scoped.rendered_body()),
        "the body is JSON",
    );
    assert_eq!(scoped_body["labelIds"], serde_json::json!(["INBOX"]));
    assert_eq!(scoped_body["labelFilterBehavior"], "include");
    // And the deprecated spelling is still absent in the filtered case, which is the one where sending it would
    // look most plausible.
    assert_eq!(scoped_body.get("labelFilterAction"), None);
    let excluded = must(
        gmail_watch(
            "projects/p/topics/t",
            Some(&["SPAM".to_owned()]),
            Some(crate::google::client::LabelFilterBehavior::Exclude),
        ),
        "an excluding watch must build",
    );
    let excluded_body: serde_json::Value = must(
        serde_json::from_str(excluded.rendered_body()),
        "the body is JSON",
    );
    assert_eq!(excluded_body["labelFilterBehavior"], "exclude");
}

#[test]
fn a_label_filter_with_nothing_to_filter_is_refused_because_the_provider_would_succeed_anyway() {
    // **The silent case this variant exists for.** `labelFilterBehavior` is documented as the "filtering
    // behavior of `labelIds` list specified", so with no list it governs nothing — and the provider does not
    // treat a filter with no list as an error, it simply watches every change. So the failure would be a
    // connector that asked for some labels and received all of them, with no error anywhere to notice.
    // Refused here because this is the only layer that can catch it.
    assert_eq!(
        gmail_watch(
            "projects/p/topics/t",
            None,
            Some(crate::google::client::LabelFilterBehavior::Include)
        ),
        Err(RequestError::Ignored {
            field: "label_filter_behavior",
            reason: "it filters a `label_ids` list, so with no labels it governs nothing and the provider \
                     watches every change; send label ids, or omit the filter",
        }),
        "a filter with no list must be refused rather than sent"
    );
    // The control: the SAME behaviour is accepted as soon as a list is present, so the refusal is about the
    // pairing and not about the behaviour being unacceptable.
    assert!(
        gmail_watch(
            "projects/p/topics/t",
            Some(&["INBOX".to_owned()]),
            Some(crate::google::client::LabelFilterBehavior::Include)
        )
        .is_ok(),
        "the same filter must be accepted when it has a list to govern"
    );
}

#[test]
fn an_empty_label_list_is_refused_rather_than_read_as_no_filter() {
    // An empty list and an absent list render to the same body, so accepting the empty one would equate a value
    // a caller built by mistake (a loop over zero labels) with a deliberate choice. "No filter" is expressed by
    // passing `None`; the two situations must not render the same.
    assert_eq!(
        gmail_watch("projects/p/topics/t", Some(&[]), None),
        Err(RequestError::Argument {
            field: "label_ids",
            reason: "an empty label list means \"no filter\", which is what omitting the argument \
                     says; pass no labels rather than an empty list, so a filter the caller did not \
                     choose cannot render as one they did",
        })
    );
    // The control: an absent list still builds the unfiltered watch, so the refusal above is about the empty
    // list and not about the unfiltered case being unreachable.
    assert!(gmail_watch("projects/p/topics/t", None, None).is_ok());
}

#[test]
fn a_watch_topic_name_goes_through_the_same_bounds_as_every_other_resource_identifier() {
    // The topic name becomes a request body field and a log line, so it is bounded like a resource id: empty,
    // whitespace-only, control-bearing and oversized are each refused, and the field is named so a caller knows
    // which argument to fix.
    for (topic, what) in [
        ("", "an empty topic name"),
        ("   ", "a whitespace-only topic name"),
        ("projects/p/topics/t\n", "a control character"),
    ] {
        assert!(
            matches!(
                gmail_watch(topic, None, None),
                Err(RequestError::Argument {
                    field: "topic_name",
                    ..
                })
            ),
            "{what} must be refused as the topic name"
        );
    }
    assert!(
        matches!(
            gmail_watch(&"a".repeat(MAX_TOPIC_NAME_CHARS + 1), None, None),
            Err(RequestError::Argument {
                field: "topic_name",
                ..
            })
        ),
        "an oversized topic name must be refused"
    );
    // And a label id goes through the same `resource_id` validator as every other identifier, including the
    // empty case, so the new request body is not a hole in the validation the query parameters go through.
    assert!(
        matches!(
            gmail_watch("projects/p/topics/t", Some(&[String::new()]), None),
            Err(RequestError::Argument {
                field: "label_ids",
                ..
            })
        ),
        "an empty label id must be refused by the shared resource-id validator"
    );
    // The list's own cap, which is a JARVIS bound rather than a provider figure.
    assert!(
        matches!(
            gmail_watch(
                "projects/p/topics/t",
                Some(&vec!["INBOX".to_owned(); MAX_WATCH_LABEL_IDS + 1]),
                None
            ),
            Err(RequestError::Argument {
                field: "label_ids",
                ..
            })
        ),
        "a label list over the cap must be refused"
    );
}

#[test]
fn a_calendar_channel_stop_names_the_two_identifiers_the_reference_requires() {
    // The `channels.stop` reference gives a body of exactly `id` and `resourceId` — *"This method requires that
    // you provide at least the channel's `id` and the `resourceId` properties"* — and the two are opaque
    // strings of similar shape, so a **transposition would be well-formed and would stop the wrong channel or
    // none**. Parsed rather than substring-matched, because the substance is which field holds which value.
    let request = must(
        calendar_channel_stop("channel-alpha", "o3hgv1538sdjfh"),
        "a stop with both identifiers must build",
    );
    // The address is the single `channels/stop` path on the Calendar base -- not a per-user or per-calendar
    // path, which is what makes this one call per channel rather than one call per account.
    assert_eq!(
        request.url(),
        "https://www.googleapis.com/calendar/v3/channels/stop"
    );
    assert_eq!(request.content_type(), "application/json");
    let body: serde_json::Value = must(
        serde_json::from_str(request.rendered_body()),
        "the body is JSON",
    );
    assert_eq!(body["id"], "channel-alpha");
    assert_eq!(body["resourceId"], "o3hgv1538sdjfh");
    // Exactly two fields: the reference lists an optional `token`, and sending the channel token back would put
    // the anti-spoofing control into a body that a diagnostic renders -- for no effect, since the field does not
    // stop anything.
    assert_eq!(
        body.as_object().map(serde_json::Map::len),
        Some(2),
        "the stop body must carry the two identifiers and nothing else: {body}"
    );
    assert_eq!(body.get("token"), None);
    // And the transposition the parse above rules out is checked once more by value, because `id` and
    // `resourceId` are both opaque and the assertion above would pass for a body whose fields were swapped only
    // if the two arguments were swapped with them.
    assert_ne!(body["id"], body["resourceId"]);
}

#[test]
fn both_channel_identifiers_go_through_the_same_identifier_validator() {
    // The stop body is a second body on a second API, so the risk is that it acquires its own weaker
    // validation. Each argument is checked against the same `resource_id` rules every other identifier here
    // follows -- empty, whitespace-only, control-bearing and oversized -- and the **field is named**, so a
    // caller learns which of the two is unusable rather than being told "the request is invalid".
    for (channel, resource, field) in [
        ("", "o3hgv1538sdjfh", "channel_id"),
        ("channel-alpha", "", "resource_id"),
        ("   ", "o3hgv1538sdjfh", "channel_id"),
        ("channel-alpha", "   ", "resource_id"),
        ("chan\nnel", "o3hgv1538sdjfh", "channel_id"),
        ("channel-alpha", "res\tource", "resource_id"),
    ] {
        assert!(
            matches!(
                calendar_channel_stop(channel, resource),
                Err(RequestError::Argument { field: named, .. }) if named == field
            ),
            "`{channel}` / `{resource}` must be refused naming `{field}`"
        );
    }
    let oversized = "a".repeat(MAX_RESOURCE_ID_CHARS + 1);
    assert!(
        matches!(
            calendar_channel_stop(&oversized, "o3hgv1538sdjfh"),
            Err(RequestError::Argument {
                field: "channel_id",
                ..
            })
        ),
        "an oversized channel id must be refused"
    );
    assert!(
        matches!(
            calendar_channel_stop("channel-alpha", &oversized),
            Err(RequestError::Argument {
                field: "resource_id",
                ..
            })
        ),
        "an oversized resource id must be refused"
    );
    // The control: the boundary value itself is accepted, so the refusals above are about being outside the
    // bound rather than about the bound being unreachable.
    assert!(calendar_channel_stop("channel-alpha", &"b".repeat(MAX_RESOURCE_ID_CHARS)).is_ok());
}
