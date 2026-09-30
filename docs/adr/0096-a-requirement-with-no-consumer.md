# ADR-0096: A requirement with no consumer, and a scope justified by the wrong operation

- **Status:** Accepted
- **Date:** 2026-09-30
- **Slice:** `P5-005` (the Google connector contract — the account-identity read).
- **Relates to:** `ADR-0083` (a declared output field is bounded by what the request can return — the defect
  this ADR avoids in the *new* schema, and the one it checks the renderer against), `ADR-0092` (a response field
  with no reader — the same "declared but unread" shape, on a field rather than an operation), `ADR-0091` (the
  mailbox address is redacted because it is a person's), and `P5-004`'s research record, whose JARVIS Mapping
  names `users.getProfile` as the source of the account identity.

## Context

`tools-and-connectors.md` requires an account's identity to be "verified from the provider, not user-entered
labels", and `P5-004`'s mapping table records how Google satisfies it:

> | Gmail account identity | `VerifiedAccount` from `users.getProfile` | `emailAddress` is provider-verified,
> not user-entered |

The connector had **no operation** that read a profile. Its four declared operations were `gmail_messages_list`,
`gmail_history_read`… `gmail_messages_read`, `gmail_history_list` and `calendar_events_read` — all of which
return *content* — and none of which says **which mailbox answered**. So:

- `crate::account::VerifiedAccount::new` requires a `provider_account_id`, and nothing in this connector could
  produce one. The requirement was **unimplementable**, not merely unimplemented.
- The gap was invisible because it was an **absence**: no operation to call, no field rendered wrongly, no test
  failing. A reader checking "is the identity verified from the provider" would find `VerifiedAccount`'s types,
  `users.getProfile` in the research record, and no reason to suspect the two were never connected.

And the connector's own comment about the scope it requests for identity was **wrong in a way that made the gap
look closed**:

> "The `users.getProfile` response carries an `emailAddress`, and that is the operation **this scope exists
> for**"

— where "this scope" is `openid`. The `users.getProfile` reference says otherwise:

> "Requires one of the following OAuth scopes: `https://mail.google.com/`,
> `https://www.googleapis.com/auth/gmail.modify`, `https://www.googleapis.com/auth/gmail.compose`,
> `https://www.googleapis.com/auth/gmail.readonly`, `https://www.googleapis.com/auth/gmail.metadata`"

**`openid` is not among them.** So the sentence described a scope as the enabler of an operation that refuses
it, and a reader who followed the comment would conclude the identity path needed nothing more — the comment
discharged the work the code had not done. The two defects are one shape: **a declaration that stood in for an
implementation** (`ADR-0092`'s "no reader" for a field, here for an operation).

`openid` is not useless — it is what makes Google return an `id_token`, which
`crate::token::TokenResponse::has_id_token` records arriving and the crate deliberately does not verify. But it
is requested for **that**, not for the profile, and the difference is the whole finding.

## Decision

1. **`gmail_profile_read` is declared and implemented**, wrapping `users.getProfile` at **1 quota unit** — the
   cheapest call in the connector, and the provider's figure. It is `ReadOnly`, risk 0, and requires the same
   `mail.read` JARVIS scope as the other Gmail reads, because Google groups it with them.

2. **The request builder takes no arguments and hardcodes `me`.** The reference documents the path parameter as
   *"The user's email address. The special value `me` can be used to indicate the authenticated user."* A
   `user_id` argument would be a field a caller could aim at another mailbox — a request its own token does not
   authorise, refused as a `403` rather than as a schema error. `gmail_profile()` therefore has no parameter and
   the input schema has `"properties": {}` with `additionalProperties: false`, so the call is unrepresentable
   rather than discouraged.

3. **The address is required and the position is not.** The response is
   `{ "emailAddress": string, "messagesTotal": integer, "threadsTotal": integer, "historyId": string }`;
   `parse_profile` refuses a response with no usable address, because the operation exists to establish *which*
   mailbox answered and a profile without one establishes nothing. **A whitespace-only address is refused too**
   — it satisfies "the field was present" while denoting nothing. `history_id` stays optional.

4. **`messagesTotal` and `threadsTotal` are deliberately not declared or rendered.** They are mailbox *counts*,
   nothing consumes them, and a schema promising a value the tool never returns is exactly `ADR-0083`'s defect.
   The test asserts the rendered output contains neither, so the omission is checked in both directions.

5. **The `SCOPE_OPENID` doc is corrected rather than deleted.** It still records what the scope *is* for — the
   `id_token`, and the `nonce` that a future ID-token check would compare — and now states plainly that
   `getProfile` does not accept it, that the Gmail read scope is what makes the profile readable, and that the
   ID-token check is **unbuilt**: a prepared seam, not a working feature.

6. **The identity is a person's address, so it is treated like the others.** The output is `Confidential` with
   the mail this connector reads, and the value is the same class that `PubsubNotification` and
   `VerifiedAccount` both redact in `Debug` (`ADR-0091`) — the tool *returns* it to a caller that asked, while
   a diagnostic does not print it.

## Consequences

- **The account-identity requirement is now satisfiable**, and the test asserts the declaration rather than a
  shape: the operation exists, carries `mail.read`, is `ReadOnly`, and costs the provider's 1 unit. A future
  edit that removed it fails with the reason it must not be removed.
- **The `openid` correction is asserted, not just written down.** One test checks that the profile operation
  does *not* carry an `openid` JARVIS scope, that the granted scopes contain both the Gmail read (which makes
  the profile answer) and `openid` (for the id token), and that the two strings differ — so a reader cannot
  conclude that requesting one grants the other. It is the `assert_ne!`-on-a-divergence shape the revocation
  module uses.
- **Two guards were falsified A-B-A**, both compiling: removing the required-address check, and removing
  `gmail_profile_read` from the operations `interpret_response` accepts. The second is the interesting one — it
  turns a declared, working operation into one that reports `NotImplemented`, which is what the missing
  operation looked like from the other side.
- **A generalisation worth keeping:** *a requirement's evidence is the operation that returns it, not the
  documentation that names it.* A comment saying "operation X satisfies requirement Y" is a claim to verify
  against the API's own scope and response tables, and here it was false about the **scope** as well as absent
  about the operation. Check that the named operation exists, and check that it accepts the credential the
  connector holds.
- **And a second:** *a doc comment can discharge work the code has not done.* The comment's presence made the
  gap read as handled, which is the same failure mode `ADR-0092` records for a reader named for one field while
  its sibling went unread. Both are cases where a **plausible statement** substituted for a **working link**.
- **A limit remains:** no request is sent and no profile has been read. Whether Google's `getProfile` returns
  what the reference says, and whether the declared `1` unit is charged, are established only by the live smoke
  test that does not exist. The `id_token` is still **received and unverified** — the new operation closes the
  *identity* requirement, not the *token-verification* one, and those are different gaps that this ADR does not
  conflate.

## Alternatives considered

- **Verify the `id_token` instead of reading the profile.** Rejected as a different slice, and worse as the
  identity path: it needs JWKS fetching (a network dependency in a path `P5-001` kept pure), and the token is
  only issued when `openid` is granted — so an account that consented to mail but not to OIDC would have no
  identity at all. `getProfile` needs only the scope the connector already has.
- **Take the address from the push notification's `emailAddress`.** Rejected: a notification arrives only after
  a watch is set up, so it cannot establish identity *at connect time*, and it is a value on a public endpoint
  rather than a response to an authenticated call.
- **Derive identity from the `email`/`profile` OIDC claims.** Rejected for the reason already recorded: the
  discovery document advertises them, but the connector would then request scopes it does not use, and the
  claims would be unverified for the same reason the `id_token` is.
- **Accept a `user_id` argument for the profile read.** Rejected: it lets a caller name another mailbox, which
  its token cannot address. The refusal would arrive as a `403` from Google rather than as an error naming the
  argument, which is the worse diagnostic for a mistake that is structural.
- **Make `email_address` optional, matching `historyId`.** Rejected: the operation exists to say which account
  answered. An optional address would let a caller treat "no identity" as a normal result, and the requirement
  it satisfies would then be satisfied by nothing.
- **Declare `messagesTotal`/`threadsTotal` since the provider returns them.** Rejected: `ADR-0083` is exactly
  this — a declared field bounded by what the request and renderer produce. They can be declared when something
  reads them.
- **Delete the `SCOPE_OPENID` constant, since its stated purpose was wrong.** Rejected: the scope *is*
  requested and does have an effect (the `id_token`), so deleting it would remove a real declaration to hide a
  false explanation. Correcting the explanation is the fix; removing the thing it described would have been
  concealment.

## Conditions that would justify revisiting

- A live smoke test confirms `getProfile`'s response shape and its `1`-unit cost, which would replace the
  transcribed figures with observed ones.
- An ID-token check is built, at which point `openid`'s justification becomes a working capability rather than
  a prepared seam, and the `nonce` carried to the grant acquires its consumer.
- A connect-time identity flow is specified — the sequence that calls `gmail_profile_read`, builds
  `VerifiedAccount`, and stores `AccountReference` — which is where this operation becomes *used* rather than
  merely *available*. Nothing calls it today, which is the same limit the push handler and the sync loop carry.
- A second provider's identity operation is added (`P5-006`, Microsoft Graph `/me`), at which point whether the
  identity read belongs in a shared shape or stays per-provider becomes live.
