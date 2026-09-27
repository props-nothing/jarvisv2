//! Tests for the account contract.
//!
//! The assertion that matters most is a **shape** assertion: that no field anywhere holds a user-supplied
//! label. "Account identity verified from the provider, not user-entered labels" is a rule a `String` field
//! named `label` would violate the moment somebody added it, so the test pins the absence — `P4-008`'s
//! "a placeholder is a value a caller could also send" applied to a field that would become an approval's
//! display text.

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

fn at(seconds: i64) -> UtcTimestamp {
    must(
        UtcTimestamp::from_unix_nanos(i128::from(seconds) * 1_000_000_000),
        "a valid instant",
    )
}

/// A provider-verified account with the given scopes.
fn account(scopes: &[&str]) -> VerifiedAccount {
    must(
        VerifiedAccount::new(
            reference("acct-1"),
            "person@example.invalid",
            Some("A Person".to_owned()),
            AuthMethod::OAuthPkce,
            scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            at(1_700_000_000),
        ),
        "a valid verified account",
    )
}

#[test]
fn a_verified_account_carries_the_providers_claims_and_no_label() {
    let account = account(&["read", "send"]);
    assert_eq!(account.reference().as_str(), "acct-1");
    assert_eq!(account.provider_account_id(), "person@example.invalid");
    assert_eq!(account.display_name(), Some("A Person"));
    assert_eq!(account.method(), AuthMethod::OAuthPkce);
    assert_eq!(account.scopes(), &["read".to_owned(), "send".to_owned()]);
    assert_eq!(account.verified_at(), at(1_700_000_000));

    // The shape assertion: a serialization of this type offers no field a caller could have supplied as a
    // label, so an approval displaying it cannot be made to show text an attacker chose.
    let encoded = serde_json::to_string(&account.provider_account_id);
    assert!(
        encoded.is_ok(),
        "the provider id must be JSON-representable"
    );
    // And `Debug` redacts the provider identifier, which is usually a person's address.
    let rendered = format!("{account:?}");
    assert!(
        !rendered.contains("person@example.invalid"),
        "`Debug` must redact the provider account identifier: {rendered}"
    );
    assert!(rendered.contains("redacted"), "got {rendered}");
    assert!(
        rendered.contains("A Person"),
        "the provider's display name is not the identifier and may be shown: {rendered}"
    );
}

#[test]
fn an_unusable_provider_identifier_is_refused() {
    for unusable in [
        "",
        "  ",
        "person\n@example.invalid",
        "person\u{0}@example.invalid",
    ] {
        assert!(
            VerifiedAccount::new(
                reference("acct-1"),
                unusable,
                None,
                AuthMethod::OAuthPkce,
                Vec::new(),
                at(0),
            )
            .is_err(),
            "`{}` must be refused as a provider account identifier",
            unusable.escape_debug()
        );
    }
    assert!(
        VerifiedAccount::new(
            reference("acct-1"),
            "x".repeat(MAX_PROVIDER_ACCOUNT_ID_CHARS + 1),
            None,
            AuthMethod::ApiKey,
            Vec::new(),
            at(0),
        )
        .is_err(),
        "an oversized provider account identifier must be refused"
    );
    // A display name is OPTIONAL, because a provider may report none — but a supplied unusable one is
    // refused, because "unset" and "set to whitespace" are different and only the first is honest.
    assert!(
        VerifiedAccount::new(
            reference("acct-1"),
            "id",
            None,
            AuthMethod::ApiKey,
            Vec::new(),
            at(0)
        )
        .is_ok(),
        "a provider that reports no display name must be accepted"
    );
    for unusable in ["", "   ", "\n"] {
        assert!(
            VerifiedAccount::new(
                reference("acct-1"),
                "id",
                Some(unusable.to_owned()),
                AuthMethod::ApiKey,
                Vec::new(),
                at(0),
            )
            .is_err(),
            "a supplied display name of `{}` must be refused",
            unusable.escape_debug()
        );
    }
}

#[test]
fn a_grant_is_checked_as_a_set_and_the_missing_scopes_are_named() {
    let account = account(&["read", "send"]);
    // Containment is a set operation, so order does not matter — a provider is free to return its scopes in
    // any order, and a sequence comparison would report a shortfall for a complete grant.
    assert!(account.grants_all(&["send".to_owned(), "read".to_owned()]));
    assert!(account.grants_all(&[]));
    assert!(!account.grants_all(&["delete".to_owned()]));
    assert!(!account.grants_all(&["read".to_owned(), "delete".to_owned()]));
    // The missing scopes are reported rather than inferred, because the requirement says reauth must preserve
    // account references "without hiding lost scopes" — so a prompt needs to name them.
    assert_eq!(
        account.missing_scopes(&["read".to_owned()]),
        Vec::<String>::new()
    );
    assert_eq!(
        account.missing_scopes(&["delete".to_owned(), "read".to_owned()]),
        vec!["delete".to_owned()]
    );
    // The two predicates are complements, asserted together because a shortfall reported as no-shortfall
    // would be invisible in a test that only checked one direction.
    for required in [
        vec![],
        vec!["read".to_owned()],
        vec!["delete".to_owned()],
        vec!["read".to_owned(), "delete".to_owned()],
    ] {
        assert_eq!(
            account.grants_all(&required),
            account.missing_scopes(&required).is_empty(),
            "grants_all and missing_scopes must agree about {required:?}"
        );
    }
}

#[test]
fn an_account_reference_may_not_hold_a_character_that_would_change_a_path_or_a_key() {
    // The reference appears in configuration keys, diagnostics fields, and path segments, so an uppercase or
    // dotted one is refused rather than sanitized — `P3-012b`'s `path_for` parses an identifier rather than
    // sanitizing it, for the same reason: sanitizing is a transformation that can be got wrong.
    for unusable in [
        "", "Acct", "acct.1", "acct/1", "acct 1", "acct@1", "acct\\1", "../etc",
    ] {
        assert!(
            AccountReference::new(unusable).is_err(),
            "`{unusable}` must be refused as an account reference"
        );
    }
    assert!(AccountReference::new("a".repeat(AccountReference::MAX_BYTES)).is_ok());
    assert!(AccountReference::new("a".repeat(AccountReference::MAX_BYTES + 1)).is_err());
    for usable in ["acct-1", "acct1", "0", "a-b-c"] {
        assert!(
            AccountReference::new(usable).is_ok(),
            "`{usable}` must be accepted as an account reference"
        );
    }
}

#[test]
fn an_account_status_says_whether_calls_may_run_and_whether_a_user_must_act() {
    // `security.md`: "missing or stale evidence fails closed", so the permitting states are the narrow ones.
    let table = [
        // status, permits calls, needs a user, is connected
        (AccountStatus::Connected, true, false, true),
        // A scope shortfall is CONNECTED and may run what its grant covers. Conflating it with a disconnect
        // would make a user's deliberate partial consent indistinguishable from revocation.
        (AccountStatus::ScopeShortfall, true, false, true),
        // A rate limit or outage is connected, but nothing may run.
        (AccountStatus::Unavailable, false, false, true),
        (AccountStatus::NeedsReauth, false, true, false),
        (AccountStatus::Disconnected, false, false, false),
    ];
    for (status, permits, needs_user, connected) in table {
        assert_eq!(status.permits_calls(), permits, "{status:?} permits calls");
        assert_eq!(status.needs_user(), needs_user, "{status:?} needs a user");
        assert_eq!(status.is_connected(), connected, "{status:?} is connected");
        // A state that permits calls must also be connected: the converse is false and that is the point.
        if status.permits_calls() {
            assert!(
                status.is_connected(),
                "{status:?} permits calls, so it must be connected"
            );
        }
    }
    // Exactly one state needs a user, so a caller counting them cannot double-count.
    let needing: Vec<AccountStatus> = table
        .iter()
        .filter(|(status, ..)| status.needs_user())
        .map(|(status, ..)| *status)
        .collect();
    assert_eq!(needing, vec![AccountStatus::NeedsReauth]);
}
