//! Tests for the sync cursor.
//!
//! The load-bearing tests are the two shape rules — a `start` cursor carries no token and every other kind
//! carries one — because those are what keep "never synced" distinguishable from "synced to here". A
//! design that allowed both would make a full resync the default a caller stumbles into, and the cost of
//! that mistake is a provider's whole history.

use super::*;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn reference(value: &str) -> AccountReference {
    must(AccountReference::new(value), "a valid account reference")
}

fn version(value: &str) -> crate::manifest::ConnectorVersion {
    must(
        crate::manifest::ConnectorVersion::new(value),
        "a valid connector version",
    )
}

fn at(seconds: i64) -> UtcTimestamp {
    must(
        UtcTimestamp::from_unix_nanos(i128::from(seconds) * 1_000_000_000),
        "a valid instant",
    )
}

#[test]
fn a_start_cursor_is_distinguishable_from_a_position() {
    // The module's central decision. "Never synced" and "synced to here" are different values, so a full
    // sync is a decision the caller makes rather than the default it falls into.
    let start = SyncCursor::start(reference("acct-1"), &version("1.0.0"), at(0));
    assert_eq!(start.kind(), SyncCursorKind::Start);
    assert_eq!(start.token(), None);
    assert!(start.requires_full_resync());
    // And a `start` cursor may NOT carry a token, which is what keeps the two distinguishable.
    match SyncCursor::new(
        SyncCursorKind::Start,
        Some("token".to_owned()),
        reference("acct-1"),
        "1.0.0",
        at(0),
    ) {
        Err(CursorError::Shape { reason }) => {
            assert!(reason.contains("never synced"), "got {reason}");
        }
        other => panic!("a start cursor with a token must be refused, got {other:?}"),
    }
    // Conversely, no other kind may omit one.
    for kind in [
        SyncCursorKind::OpaqueToken,
        SyncCursorKind::MonotonicMarker,
        SyncCursorKind::Etag,
        SyncCursorKind::Offset,
    ] {
        assert!(
            matches!(
                SyncCursor::new(kind, None, reference("acct-1"), "1.0.0", at(0)),
                Err(CursorError::Shape { .. })
            ),
            "{kind:?} must require a token"
        );
        let cursor = must(
            SyncCursor::new(
                kind,
                Some("token".to_owned()),
                reference("acct-1"),
                "1.0.0",
                at(0),
            ),
            "a cursor with a token",
        );
        assert!(!cursor.requires_full_resync(), "{kind:?} has a position");
        assert_eq!(cursor.kind(), kind);
    }
}

#[test]
fn a_cursor_token_may_not_carry_a_control_character_or_be_unbounded() {
    // The token becomes a request parameter and a stored field, and a newline in either forges a record.
    for unusable in ["", "tok\nen", "tok\r\nen", "tok\u{0}en"] {
        assert!(
            matches!(
                SyncCursor::new(
                    SyncCursorKind::OpaqueToken,
                    Some(unusable.to_owned()),
                    reference("acct-1"),
                    "1.0.0",
                    at(0),
                ),
                Err(CursorError::Token { .. })
            ),
            "`{}` must be refused as a cursor token",
            unusable.escape_debug()
        );
    }
    assert!(
        SyncCursor::new(
            SyncCursorKind::OpaqueToken,
            Some("x".repeat(MAX_SYNC_CURSOR_CHARS + 1)),
            reference("acct-1"),
            "1.0.0",
            at(0),
        )
        .is_err()
    );
    assert!(
        SyncCursor::new(
            SyncCursorKind::OpaqueToken,
            Some("x".repeat(MAX_SYNC_CURSOR_CHARS)),
            reference("acct-1"),
            "1.0.0",
            at(0),
        )
        .is_ok()
    );
    // A version is required, because a cursor from a previous format is not applicable to the current one —
    // and an empty one would make that comparison always false, so every cursor would look applicable.
    assert!(matches!(
        SyncCursor::new(
            SyncCursorKind::OpaqueToken,
            Some("token".to_owned()),
            reference("acct-1"),
            "  ",
            at(0),
        ),
        Err(CursorError::Shape { .. })
    ));
}

#[test]
fn a_cursor_is_applicable_only_to_the_account_and_version_that_produced_it() {
    // Both halves matter and neither is visible in the token: a provider that answers a foreign token
    // *differently* rather than refusing it is the case a caller cannot detect unaided.
    let cursor = must(
        SyncCursor::new(
            SyncCursorKind::OpaqueToken,
            Some("token".to_owned()),
            reference("acct-1"),
            "1.0.0",
            at(1_700_000_000),
        ),
        "a cursor",
    );
    assert!(cursor.applies_to(&reference("acct-1"), "1.0.0"));
    assert!(
        !cursor.applies_to(&reference("acct-2"), "1.0.0"),
        "a cursor must not be applicable to another account"
    );
    assert!(
        !cursor.applies_to(&reference("acct-1"), "1.1.0"),
        "a cursor must not be applicable to another connector version"
    );
    assert!(!cursor.applies_to(&reference("acct-2"), "1.1.0"));
}

#[test]
fn the_cursor_kind_says_what_may_be_concluded_from_it() {
    // The whole reason the kind exists: a monotonic marker's staleness is detectable and has a defined
    // recovery, while an opaque token's is not — treating one as the other invents a comparison the provider
    // never offered.
    assert!(SyncCursorKind::MonotonicMarker.can_be_detected_as_stale());
    assert!(SyncCursorKind::Etag.can_be_detected_as_stale());
    assert!(
        !SyncCursorKind::OpaqueToken.can_be_detected_as_stale(),
        "an opaque token's staleness arrives as a provider error, not as a detectable condition"
    );
    assert!(!SyncCursorKind::Offset.can_be_detected_as_stale());
    assert!(!SyncCursorKind::Start.can_be_detected_as_stale());

    // Reconstruction is possible only for an offset, and that is precisely why it is the dangerous kind: a
    // deleted item shifts everything after it.
    for kind in [
        SyncCursorKind::OpaqueToken,
        SyncCursorKind::MonotonicMarker,
        SyncCursorKind::Etag,
    ] {
        assert!(
            kind.needs_full_resync_when_lost(),
            "{kind:?} cannot be reconstructed"
        );
    }
    assert!(!SyncCursorKind::Offset.needs_full_resync_when_lost());
    assert!(!SyncCursorKind::Start.needs_full_resync_when_lost());
}

#[test]
fn a_sync_window_is_bounded_at_both_ends() {
    // A connector that synced a year of mail because a cursor was lost would exhaust a rate-limit budget and
    // produce a backlog nobody can review, so the window is required rather than defaulted.
    for (items, seconds) in [
        (0, 60),
        (SyncWindow::MAX_ITEMS_CEILING + 1, 60),
        (100, 0),
        (100, SyncWindow::MAX_SECONDS_CEILING + 1),
        (0, 0),
    ] {
        assert!(
            matches!(
                SyncWindow::new(Some(at(0)), items, seconds),
                Err(CursorError::Window { .. })
            ),
            "a window of {items} items in {seconds} seconds must be refused"
        );
    }
    let bounded = must(SyncWindow::new(Some(at(0)), 100, 60), "a bounded window");
    assert!(!bounded.is_full_resync());
    assert_eq!(bounded.max_items, 100);
    assert_eq!(bounded.max_seconds, 60);
    // The ceiling itself is accepted, so the check is a boundary rather than an off-by-one.
    assert!(
        SyncWindow::new(
            None,
            SyncWindow::MAX_ITEMS_CEILING,
            SyncWindow::MAX_SECONDS_CEILING
        )
        .is_ok()
    );
    // A window with no start is a full resync, reported rather than inferred, because it is the expensive
    // case a caller logs.
    let full = must(SyncWindow::new(None, 100, 60), "a full window");
    assert!(full.is_full_resync());
}

#[test]
fn the_cursor_parts_travel_with_what_they_denote() {
    // `P3-006a`'s pattern: a token and the account it came from are two values that must agree, so they are
    // assembled through one constructor rather than paired at each call site.
    let cursor = must(
        SyncCursor::new(
            SyncCursorKind::Etag,
            Some("\"abc123\"".to_owned()),
            reference("acct-1"),
            "1.0.0",
            at(1_700_000_000),
        ),
        "a cursor",
    );
    let parts: SyncCursorParts = cursor.clone().into();
    assert_eq!(parts.kind, SyncCursorKind::Etag);
    assert_eq!(parts.account, reference("acct-1"));
    let reassembled = must(parts.assemble(), "the parts must reassemble");
    assert_eq!(reassembled, cursor);
    // And a part set that violates a shape rule is refused on the way back, so a decoded cursor is validated
    // exactly as one built in code.
    let malformed = SyncCursorParts {
        kind: SyncCursorKind::Start,
        token: Some("unexpected".to_owned()),
        account: reference("acct-1"),
        connector_version: "1.0.0".to_owned(),
        observed_at: at(0),
    };
    assert!(matches!(
        malformed.assemble(),
        Err(CursorError::Shape { .. })
    ));
    // The accessors expose what an adapter needs, including the instant — the age is what "is this cursor
    // still usable" reads.
    assert_eq!(cursor.token(), Some("\"abc123\""));
    assert_eq!(cursor.account(), &reference("acct-1"));
    assert_eq!(cursor.connector_version(), "1.0.0");
    assert_eq!(cursor.observed_at(), at(1_700_000_000));
}

#[test]
fn the_cursor_parts_do_not_print_the_token_the_cursor_hides() {
    // **The redaction has to follow the value, and `into()` is where it did not.** `SyncCursor` hides its
    // token, but `From<SyncCursor> for SyncCursorParts` moves the token into a second type that derived
    // `Debug` — so converting and printing printed what the redaction two definitions up was for. This test
    // is the crate's marker-plus-control shape, and it asserts the *parts* because that is the type that
    // leaked (`ADR-0091`).
    let cursor = must(
        SyncCursor::new(
            SyncCursorKind::MonotonicMarker,
            Some("9876543210".to_owned()),
            reference("acct-1"),
            "1.0.0",
            at(1_700_000_000),
        ),
        "a cursor",
    );
    let parts: SyncCursorParts = cursor.into();
    let rendered = format!("{parts:?}");
    assert!(
        !rendered.contains("9876543210"),
        "the parts must not print the token the cursor redacts: {rendered}"
    );
    assert!(
        rendered.contains("[REDACTED]"),
        "the token must be redacted rather than omitted, so a reader sees a value was hidden: {rendered}"
    );
    // The control: the fields a diagnostic actually needs are still printed. A `Debug` that redacted
    // everything would pass the assertion above and make the type useless to debug.
    assert!(
        rendered.contains("acct-1")
            && rendered.contains("MonotonicMarker")
            && rendered.contains("1.0.0"),
        "the non-sensitive fields must still be printed: {rendered}"
    );
    // And a start cursor's absent token prints as `None`, not as a marker — see `SyncCursor`'s own test for
    // why the two situations must render differently. Reached through the validating constructor, so the
    // parts are the ones a `start` cursor actually produces.
    let start = must(
        SyncCursor::new(
            SyncCursorKind::Start,
            None,
            reference("acct-1"),
            "1.0.0",
            at(1_700_000_000),
        ),
        "a start cursor",
    );
    let start_parts: SyncCursorParts = start.into();
    let start_rendered = format!("{start_parts:?}");
    assert!(
        start_rendered.contains("token: None"),
        "an absent token must print as `None` rather than as a redaction: {start_rendered}"
    );
}
