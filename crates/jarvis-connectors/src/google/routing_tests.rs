//! Tests for routing a push delivery to a known account.
//!
//! The load-bearing ones are the **three ways routing fails**, because each has a different cause and only one
//! of them is fixed by waiting. And every failure must acknowledge rather than refuse, because a refusal is
//! charged to the whole subscription.
//!
//! Falsification record in `TODO.md`.

use super::*;
use crate::auth::AuthMethod;
use jarvis_core::UtcTimestamp;

fn must<T, E: std::fmt::Display>(result: Result<T, E>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{what}: {error}"),
    }
}

fn reference(value: &str) -> AccountReference {
    must(AccountReference::new(value), "a valid account reference")
}

fn instant(seconds: i64) -> UtcTimestamp {
    must(
        UtcTimestamp::from_unix_nanos(i128::from(seconds) * 1_000_000_000),
        "a representable instant",
    )
}

fn account(reference_value: &str, address: &str) -> VerifiedAccount {
    must(
        VerifiedAccount::new(
            reference(reference_value),
            address,
            None,
            AuthMethod::OAuthPkce,
            vec!["https://www.googleapis.com/auth/gmail.readonly".to_owned()],
            instant(1_700_000_000),
        ),
        "a verified account",
    )
}

fn notification(address: &str) -> PubsubNotification {
    PubsubNotification {
        email_address: address.to_owned(),
        history_id: "1234567890".to_owned(),
    }
}

#[test]
fn an_exact_match_is_the_only_route_applied_without_a_person() {
    // The one route that needs no human decision, and `account()` returns it so the caller does not search
    // again — a second lookup would be a second place the comparison is decided.
    let accounts = [
        account("acct-1", "person@example.invalid"),
        account("acct-2", "other@example.invalid"),
    ];
    let route = route_delivery(&notification("other@example.invalid"), &accounts);
    assert_eq!(route.account(), Some(&reference("acct-2")));
    assert!(route.may_be_applied_automatically());
    assert_eq!(route.matched_accounts(), Some(1));
    // And a routable delivery has NO pre-emptive acknowledgement: an exact route means it *can* be processed,
    // not that it was, so answering here would claim a sync that has not happened.
    assert_eq!(route.unroutable_acknowledgement(), None);
}

#[test]
fn the_comparison_is_byte_exact_so_a_neighbouring_address_is_not_a_match() {
    // **The security-relevant property.** The address is untrusted input from an endpoint this connector has
    // **no verifier for** (Finding 1: neither Google mechanism is a body MAC; the contract can now *name* the
    // two header-token schemes, but nothing compares them), so a comparison that matched loosely would let one
    // delivery choose another mailbox's account. None of these is the stored address.
    let accounts = [account("acct-1", "person@example.invalid")];
    for near_miss in [
        // A different local part on the same domain.
        "other@example.invalid",
        // A different domain with the same local part.
        "person@example.invalid.evil",
        // A **prefix**, which a `starts_with` comparison would accept.
        "person@example.inval",
        // A **superstring**, which a `contains` comparison would accept.
        "x-person@example.invalid",
        // Leading or trailing whitespace, which a trimming comparison would accept.
        " person@example.invalid",
        "person@example.invalid ",
    ] {
        let route = route_delivery(&notification(near_miss), &accounts);
        assert_eq!(
            route,
            DeliveryRoute::Unknown,
            "`{near_miss}` is not the stored address and must not route to its account"
        );
    }
    // The control: the exact value DOES route, so the refusals above are about the comparison and not about
    // the lookup being broken.
    assert!(
        route_delivery(&notification("person@example.invalid"), &accounts)
            .may_be_applied_automatically()
    );
}

#[test]
fn a_case_differing_address_is_reported_rather_than_taken() {
    // **A near-match is not a match, and this is the decision rather than an oversight.** Addresses are
    // case-insensitive in practice, so `Person@example.invalid` is *probably* the same mailbox — but Google
    // publishes no canonicalisation statement for `emailAddress` in a push payload, and if the two spellings
    // were genuinely two accounts then applying the route would read the wrong mailbox. So it is reported.
    let accounts = [account("acct-1", "person@example.invalid")];
    let route = route_delivery(&notification("Person@Example.Invalid"), &accounts);
    assert_eq!(route, DeliveryRoute::CaseDiffers { accounts: 1 });
    assert!(
        !route.may_be_applied_automatically(),
        "a case-only near-match must not be applied without a person"
    );
    // `account()` is `None` rather than returning the near-match, so no accessor can hand out an arbitrarily
    // chosen account from this variant.
    assert_eq!(route.account(), None);
    assert_eq!(route.matched_accounts(), Some(1));
}

#[test]
fn several_accounts_with_one_address_are_ambiguous_and_no_account_is_chosen() {
    // Reachable when a reconnect mints a new reference without retiring the old row. Picking one — first,
    // oldest, most recently verified — would be an arbitrary choice that decides which mailbox is read, and a
    // wrong pick syncs one mailbox's changes under another account's identity. So only a count is reported.
    let accounts = [
        account("acct-1", "person@example.invalid"),
        account("acct-2", "person@example.invalid"),
        account("acct-3", "other@example.invalid"),
    ];
    let route = route_delivery(&notification("person@example.invalid"), &accounts);
    assert_eq!(route, DeliveryRoute::Ambiguous { accounts: 2 });
    assert_eq!(
        route.account(),
        None,
        "no account may be selected from an ambiguous route: {route:?}"
    );
    assert!(!route.may_be_applied_automatically());
    assert_eq!(route.matched_accounts(), Some(2));
}

#[test]
fn every_unroutable_delivery_is_acknowledged_rather_than_refused() {
    // **The conclusion `ADR-0094` forces.** None of the three unroutable cases is repaired by another attempt —
    // the account set is a local fact — so refusing would be a negative acknowledgement that never becomes a
    // positive one. And the push page says a negative acknowledgement triggers a **subscription-global**
    // backoff of up to 60 seconds, so that refusal would slow every other mailbox on the subscription.
    let accounts = [account("acct-1", "person@example.invalid")];
    let duplicate = [
        account("acct-1", "person@example.invalid"),
        account("acct-2", "person@example.invalid"),
    ];
    let unroutable = [
        route_delivery(&notification("nobody@example.invalid"), &accounts),
        route_delivery(&notification("person@example.invalid"), &duplicate),
        route_delivery(&notification("Person@example.invalid"), &accounts),
    ];
    for route in &unroutable {
        assert_eq!(
            route.unroutable_acknowledgement(),
            Some(DeliveryAck::AbandonAndAcknowledge),
            "{route:?} must be acknowledged and recorded, not refused"
        );
        assert!(!route.may_be_applied_automatically());
    }
    // And the acknowledgement refuses nothing, which is the property the whole arm exists for: `Retry` is the
    // only answer that costs the subscription, and no unroutable case produces it.
    for route in &unroutable {
        assert!(
            route
                .unroutable_acknowledgement()
                .is_some_and(DeliveryAck::acknowledges),
            "{route:?} must not refuse the delivery"
        );
    }
}

#[test]
fn no_connected_accounts_and_an_unknown_address_are_the_same_route() {
    // Both mean "nothing here to route to", and the caller's action is identical — acknowledge and record. A
    // separate variant for "not configured" would be a state the caller cannot act on differently, so they are
    // deliberately the same answer rather than two.
    assert_eq!(
        route_delivery(&notification("person@example.invalid"), &[]),
        DeliveryRoute::Unknown
    );
    assert_eq!(
        route_delivery(
            &notification("person@example.invalid"),
            &[account("acct-1", "other@example.invalid")]
        ),
        DeliveryRoute::Unknown
    );
    // `matched_accounts` is `None` rather than `Some(0)`: zero is the absence of a count, not a count of zero.
    assert_eq!(DeliveryRoute::Unknown.matched_accounts(), None);
}

#[test]
fn routing_names_an_account_to_read_and_never_a_credential() {
    // **The property that bounds what a forged delivery can do.** What this module returns is an
    // `AccountReference` — JARVIS's own local identifier — and nothing here holds or returns a token. A forged
    // delivery can therefore only make the connector *read* a mailbox it was already authorised to read, and
    // authorization was settled when the account was connected. Asserted as a type-level fact through the
    // accessor's return type, and as the reason the module has no field that could hold a credential.
    let accounts = [account("acct-1", "person@example.invalid")];
    let route = route_delivery(&notification("person@example.invalid"), &accounts);
    let named: &AccountReference = route
        .account()
        .unwrap_or_else(|| panic!("an exact route names an account"));
    assert_eq!(named.as_str(), "acct-1");
    // And the account's own address is the untrusted value being MATCHED, never produced as an output — the
    // reference is what travels, which is why the reference is opaque and case-constrained.
    assert!(!named.as_str().contains('@'));
}
