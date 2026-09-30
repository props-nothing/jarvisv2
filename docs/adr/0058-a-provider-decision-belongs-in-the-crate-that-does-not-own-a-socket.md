# ADR-0058: A provider decision belongs in the crate that does not own a socket

- **Status:** Accepted
- **Date:** 2026-09-27
- **Slice:** `P5-005` (the Google client).
- **Relates to:** `ADR-0054` (the connector contracts), `ADR-0057` (a declaration that cannot be made honestly).

## Context

`jarvis-connectors` has no HTTP stack. That was a consequence rather than a goal when `P5-001` created the
crate — "no external client, and no I/O" was a property of what those slices happened to contain — and this
slice is where it became a decision, because a provider client is the thing that must send a request.

The tension is real in both directions:

- **Adding an HTTP client** would put a provider's transport into the crate whose entire value is that its rules
  are checkable as functions of their arguments. Every mapping could then be tested only by a server or a mock,
  and `AGENTS.md`'s "SDK types must not cross JARVIS domain boundaries" would have a second front: the request
  and response *types* of whichever client was chosen would become the shape the rules are written against.
- **Writing nothing** would mean the accumulated contract — the manifest, the auth flow, the discovered
  endpoints — has no behaviour at all, and `P5-005` would end at declarations.

`jarvis-models` already answers this shape: `ModelGateway` is a port, the real adapter is a separate
implementation, and the adapter's own tests drive a **scripted `Transport`** so the mapping is verified
offline. Google's client is the same problem with different vocabulary.

## Decision

**1. The provider's *decisions* live in `jarvis-connectors`; only the *transport* would need a socket.**

`crates/jarvis-connectors/src/google/client.rs` holds everything that is a pure function of a response:
status-and-reason to retry decision, body to reason, page token to next page, cursor to successor or to a
resync verdict. None of it can send a request, and that is precisely why every branch is testable without one.

**2. The Google module becomes the directory shape `repository-layout.md` documents.**

```
crates/jarvis-connectors/src/google/
|-- mod.rs             the contract: manifest, auth flow, endpoints, scopes
|-- client.rs          provider decisions: classification, pagination, cursors
|-- tests.rs           contract tests
`-- client_tests.rs    client tests
```

The layout document names `manifest.rs`/`auth.rs`/`client.rs`/`gmail.rs`/`calendar.rs`/`webhook.rs`/
`diagnostics.rs`/`tests.rs`, and says "do not force this exact file count when a connector is small. Preserve
the ownership categories." `mod.rs` is the contract file, `client.rs` the provider client; `gmail.rs` and
`calendar.rs` arrive with the operations, and splitting now would mean two files that each hold a third of a
declaration.

**3. A retry classification is a function of the provider's own error vocabulary, not of its status codes.**

`GmailErrorReason` is a closed set of the reasons Google's error page names, and `classify` switches on the
**reason** for a `403` because four documented reasons share that status and have three different remedies. The
case that justifies the whole table is `domainPolicy`: "the domain administrators have disabled Gmail apps"
arrives as `403`, exactly like the two throttling reasons, and its remedy is a conversation with an
administrator rather than a retry.

**4. An unknown reason is representable and classified conservatively.**

`GmailErrorReason::Unrecognised` exists because Google adds reasons, and refusing to parse an unknown one would
turn "a new error code" into "a connector that cannot read its own errors". Its classification is the
**status's**, so an unknown reason can never *loosen* a decision. For a `403` that means `Permanent` with
`DoNotRetry`, on the argument that a `403` is a refusal with **no effect** — so the honest reading of an
unknown `403` is "refused" rather than `Unknown`/`Reconcile`. A new *throttling* reason appearing as a `403` is
the cost, and it fails in the direction that cannot cause a second effect.

**5. The error body type has no field for the error message.**

`GmailErrorBody` holds a code and the reasons. There is no `message` field, so the classification cannot be
derived from message text even by accident — the structural form of `P3-008c`'s "DO NOT DERIVE A SAFETY FLAG
FROM MESSAGE TEXT", the same technique `McpToolListing` uses by having no field for a server's annotations.

**6. A stale cursor is detected by a named predicate applied to one method, never by a status code alone.**

Gmail signals a pruned history with an ordinary **HTTP 404**, which is also what a missing message returns.
`gmail_history_status_is_pruned` says `history` in its name because the caller must apply it to
`users.history.list` only, and applying it to `messages.get` would discard a whole sync over one absent message.

**7. A monotonic cursor may not move backwards; an opaque one may not be compared at all.**

`advance_gmail_history` refuses a `historyId` smaller than the cursor's, because `historyId` increases and a
smaller value is a stale or foreign response — storing it would silently re-walk history the connector has
already processed, which for a connector that acts on changes is a repeat. `advance_calendar_sync` performs no
such check, because `nextSyncToken` is opaque: comparing two would be inventing a property the provider never
offered, which is exactly what `SyncCursorKind::OpaqueToken` documents.

## Consequences

- **Every mapping is testable offline, and none of it is tested against Google.** The fixtures are built from
  the research record rather than captured from the wire, so the tests prove the code implements the record and
  prove nothing about whether the record matches Google. The manifest's
  `CompatibilityVerdict::Unverified` is the declaration of that exact gap, and it is now carrying two slices'
  worth of unverified claims.
- **`cargo deny` is unaffected**, because no dependency was added: `serde` was already a dependency of this
  crate for the manifest, and `thiserror` for its error types.
- **The transport binding is now a named, bounded step** rather than an open question: it needs an injected
  transport (the `jarvis-models` shape), a token source, and a decision about whether the binding belongs in
  this crate or in a composition root. `repository-layout.md` gives the second answer for a socket, which is
  why this slice did not pre-empt it.
- **Two provider facts are now constants rather than prose** — `GMAIL_BATCH_LIMIT` (50) and
  `GMAIL_MAX_RESULTS_CAP` (500) — and a test asserts the first is below the second, because batching is what
  makes a full sync affordable *and* is itself a rate-limit trigger. Confusing the two would ask for 500
  sub-requests at once.

  > **Correction (ADR-0080, 2026-09-30).** `GMAIL_BATCH_LIMIT = 50` was **wrong in name and value**: the batch
  > reference states a **hard limit of 100** and *recommends* no more than 50 to avoid throttling, so one
  > constant conflated a refusal with a slowdown and was set to the recommendation while being documented as
  > "the largest batch Gmail accepts". It is now `GMAIL_BATCH_HARD_LIMIT` (100) and `GMAIL_BATCH_RECOMMENDED`
  > (50). The figure was also attributed to the quota page, which states no batch limit; the batch page is now
  > in the research record's source table.

## Limits

- **No request has been sent.** `classify`, `next_page`, `advance_gmail_history` and `advance_calendar_sync` are
  never called by production code, and no response has ever been parsed. The strongest statement available is
  "the record says this and the code implements the record".
- **There is no transport, no token source, and no operation.** Nothing builds an authorization request, nothing
  exchanges a code, nothing calls `users.messages.list`. The endpoints are constants and the flow is
  constructible; neither has been used.
- **`GOOGLE_MAX_BACKOFF_SECONDS` has no caller and is not carried by any decision**, because
  `RetryGuidance::BackoffSeconds` states a *starting* delay. A retry loop that honoured a ceiling would need a
  field the shared type does not have, and adding one for a single provider would put a Google fact into
  JARVIS vocabulary. Recorded so the constant is not mistaken for enforcement.
- **The `Refused` variant of `SyncAdvance` is never constructed.** Both advance functions return `Advanced` or
  an error, so the variant that carries a classified decision for a *refusal* is unbuilt — it is the shape the
  transport binding needs, and `P5-001`'s standard applies: a variant nothing constructs reads as a live
  condition.
- **`classify` is tested on the statuses Google documents**, so a status outside that set yields `Unknown`. That
  is deliberate, but it means the classifier's behaviour on a real novel response is untested by construction.
