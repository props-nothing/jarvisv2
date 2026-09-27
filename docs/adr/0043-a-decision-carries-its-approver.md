# ADR-0043: An approval decision carries the identity that made it

**Status:** Accepted

**Date:** 2026-09-27

## Context

`ADR-0018` gave an approval a decision record, and the record has always had five parts: the outcome, the
channel, the strength, the instant, and **who answered**. The schema stores all five — `approvals` has
`state`, `decision_channel`, `decision_strength`, `occurred_at`, and `decided_by`, with a `CHECK` that
makes them move together.

The **domain type did not**. `ApprovalDecision` held four of them, and the approver travelled as a
separate `&str` argument to `apply_decision` / `apply_verified_decision` and as a third parameter of
`record_decision`. The approver was validated on the way in and on the way out — `decode_approval` read
`decided_by`, rejected an empty one, and then **discarded it** — so nothing above storage could ever learn
who answered.

That was invisible until a caller needed it. `P3-016` linked a held call to its approval, and the step
after that is resuming the call: the resumed execution builds an `AuthorizationReceipt`, which cites an
`ApprovalCitation` containing `approver_id`. Building that citation from a decided approval was
**impossible**, and the reason was not a missing function but a value that two layers each held half of.

Two things made this worth an ADR rather than a patch:

- It is a **duplication**, not an omission. The approver was an argument at three call sites and a column
  at one, so four places could disagree about one fact. The read path checked the column and then threw it
  away, which meant the validation and the retention lived in different layers.
- The failure was invisible to every existing test, because no test had ever needed to read the approver
  back. The suite proved the self-approval refusal (which *reads* the approver) and the storage round trip
  (which *checks* the column), and neither could notice that the value never survived decoding.

## Decision

**`ApprovalDecision` carries `approver_id`, and the application functions no longer take it as an
argument.**

```text
ApprovalDecision::new(outcome, channel, strength, decided_at, approver_id) -> Result<Self, InvalidApprovalField>
ApprovalRequest::apply_decision(presented_nonce, decision)
ApprovalRequest::apply_verified_decision(decision)
record_decision(database, id, presented_nonce, decision)
```

**1. The approver is a property of the decision, not a parameter beside it.**

A decision is an **act by an identity**. "Approve" with nobody behind it is not a decision with a missing
field; it is not a decision. So the constructor **requires** the approver rather than taking an `Option`,
and it is checked at construction rather than at the writer — because the self-approval guard and the
strength floor both read it, and a value admitted as empty would let a decision pass a check that needs a
subject.

**2. The constructor validates it against the column's own bound.**

`MAX_APPROVER_ID_CHARS` is 128, which is the bound `decided_by` declares. The domain check and the schema
`CHECK` therefore agree about what is storable, so a decision the domain accepts cannot be refused by
SQLite with a message naming a `CHECK` — which reads to an operator as a database fault rather than a
caller's mistake. A tighter domain rule would be arbitrary; a looser one would defer the failure.

**3. The guard reads the approver from the decision.**

`apply_verified_decision` compares `decision.approver_id()` against the request's actor. An earlier shape
took the approver as an argument **and** stored it from a separate one, which is precisely how a caller
could record one approver while the decision it persisted named another. Reading it from the decision makes
"the identity checked" and "the identity recorded" the same value by construction.

**4. The decoder retains the value instead of discarding it.**

`decode_approval` still re-checks that `decided_by` is present — the migration's `CHECK` covers rows this
repository wrote, and a hand-edited or restored row is not covered by that argument — and now passes it
into `ApprovalDecision::new` so a decoded approval is as informative as a fresh one.

## Consequences

- **A decided approval can now supply an approver identity**, which is what a resumed call's
  `AuthorizationCitation` requires. This was the blocking gap for `P3-012c`'s remainder, and it was a
  **domain** gap rather than a call-site one — the column had always held the value.
- **Four places that described one fact became one.** The self-approval check, the write, the read, and any
  future consumer all read the same field, so there is no longer a path by which the approver that was
  checked differs from the approver that was stored.
- **A decision cannot be constructed without an approver.** `ApprovalDecision::new` returns a `Result`,
  which is an API change at every call site — and that is the honest shape, because the alternative
  (`Option`, or an empty string default) makes an identity-less decision representable and pushes the
  question onto every reader.
- **An unusable approver is `InvalidApprovalField::ApproverUnusable`**, mapped to the `decided_by` field
  name by the storage layer, so a caller learns which field failed without the message forwarding the value.

## Honest limits

- **The approver is recorded, not proof of the approver.** It is the identity the channel established, and
  the strength beside it is what says how strongly. A decision cannot be attributed to an identity the
  channel did not establish, but this field does not itself prove anything about the channel — that is what
  `decision_strength` and the approval's `required_strength` are for, and neither can be replaced by reading
  this one.
- **The value is opaque to the domain.** It is bounded and non-empty, and nothing checks that it names a
  row in `users`. `0006`'s own comment gives the reason: a server deployment's approver may be a client or a
  runtime identity that is not a `users` row, and requiring one would make those unrepresentable rather than
  unauthorized. The bound and the non-emptiness are the whole of the check.

## Alternatives considered

- **Add an accessor that reads `decided_by` from the row on demand.** Rejected: it would make a second read
  of the same fact, and the two could disagree between the decision that was recorded and the row that was
  written. The value is already in hand at every point that needs it, which is the whole argument for
  carrying rather than re-reading.
- **Make `approver_id` an `Option<String>` on the decision.** Rejected: it makes "a decision nobody made"
  representable, and every reader would have to decide what that means. It never means anything useful.
- **Keep the argument and additionally store it on the decision.** Rejected: that is the duplication being
  removed. Two values for one fact is how the read path came to validate a value it then threw away.
- **Leave it alone and have the resume path read the column directly.** Rejected: the resume path builds a
  receipt citing an approver, and a citation assembled from a second read would not be tied to the decision
  it claims to be about — the same reasoning `ADR-0021` applies to a receipt deriving from its decision.

## Related

- [ADR-0018: An approval is a decision record bound to a digest](0018-approvals-bind-to-a-digest-and-store-no-bearer-token.md) — the record this
  decision completes.
- [ADR-0021: An authorization receipt derives from its decision](0021-an-authorization-receipt-derives-from-its-decision.md) — the rule this
  decision applies to the approver: a receipt cites the decision, so the decision must hold what the
  citation needs.
