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
            calendar_events_list("primary", None, None, None, None),
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
fn a_page_token_and_a_sync_token_are_not_interchangeable() {
    // Google's sync guide says `nextSyncToken` "is present only on the very last page", while `nextPageToken`
    // continues the current walk. A caller that stored the page token as a cursor would store something that
    // expires with the walk, so the two are separate fields and a fixture carries both.
    let body = r#"{
        "items": [{"id": "event-1"}, {"id": "event-2"}],
        "nextPageToken": "page-token-1",
        "nextSyncToken": "sync-token-1"
    }"#;
    let page = must(parse_calendar_page(200, body), "a valid events page");
    assert_eq!(page.ids, ["event-1", "event-2"]);
    assert_eq!(page.next_page_token.as_deref(), Some("page-token-1"));
    assert_eq!(page.next_sync_token.as_deref(), Some("sync-token-1"));
    assert_ne!(page.next_page_token, page.next_sync_token);
}

#[test]
fn a_sync_token_absent_from_a_page_is_not_an_empty_one() {
    // The middle pages of a paginated incremental sync carry no sync token at all. Reading the absence as an
    // empty token would store a cursor that addresses nothing.
    let body = r#"{"items": [{"id": "event-1"}], "nextPageToken": "more"}"#;
    let page = must(parse_calendar_page(200, body), "a valid events page");
    assert_eq!(page.next_sync_token, None);
    assert_eq!(page.next_page_token.as_deref(), Some("more"));
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
    assert!(parse_single_id(200, "not json").is_err());
    assert!(parse_single_id(200, r#"{"threadId": "t"}"#).is_err());
    assert_eq!(
        must(
            parse_single_id(200, r#"{"id": "msg-1", "threadId": "t"}"#),
            "a valid resource"
        ),
        "msg-1"
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
        calendar_events_list("primary", None, None, Some(GMAIL_MAX_RESULTS_CAP + 1), None).is_ok()
    );
    assert!(
        calendar_events_list(
            "primary",
            None,
            None,
            Some(CALENDAR_MAX_RESULTS_CAP + 1),
            None
        )
        .is_err()
    );
    let calendar = must(
        calendar_events_list("primary", None, None, Some(1_000), None),
        "a Calendar page of 1000 is within its own cap",
    );
    assert_eq!(calendar.query()[0].1, "1000");
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
        calendar_events_list("primary", None, None, None, None),
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
