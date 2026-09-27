# ADR-0042: A decision nonce is delivered through a profile-private file, not a response

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P3-012a` made a held tool call persist a durable `ApprovalRequest`. That closed the "nothing persists an
approval" limit, but it exposed a second one immediately: **a stored approval could not be decided by
anything.**

The chain was fully implemented and could not run. `DecisionNonce` is generated when the approval is
created, `create_approval` stores only `SHA-256(nonce)` (`ADR-0018`, deliberately: "a leaked row proves a
decision happened without yielding the ability to make one"), and `record_decision` requires the
**plaintext** and compares it against that digest. Meanwhile the plaintext was generated inside
`record_hold` and dropped when that function returned, so `DecisionNonce::expose` had no production
reader at all. There was no path from "an approval is pending" to "a decision is accepted".

That makes the missing piece a **delivery channel**, and the whole difficulty is deciding where the
nonce may go. The constraint is not "somewhere convenient"; it comes from what the nonce is for.
`docs/architecture/security.md` names the threat as **model self-approval / confused deputy**, so the
nonce exists to prevent the party that *requested* an action from *approving* it. In this codebase the
requester of a tool call is the **run** — the agent (`P3-012a` made that explicit, so the domain's
`approver != requester` rule would have work to do). Therefore:

- the requester must never receive the nonce;
- the nonce must not be derivable by anything that can read the durable row;
- the nonce must survive a daemon restart, because `A05` and the Phase 3 gate both require an approval to
  outlive the process that requested it.

`ADR-0018` had already rejected one option for a reason that rules out a tempting shortcut: deriving the
nonce from a server secret "removes the one-time property, because the derived value stays valid until
the secret rotates".

## Decision

**1. The plaintext nonce is written to a profile-private file, and the row keeps only the digest.**

`crates/jarvis-storage/src/secret_store.rs` is the single writer. It is the same shape the daemon already
uses for its own client credential (`CredentialStore`): the daemon **issues** a secret into a file the
human's account can read, and the secret never travels in a request body and is never returned to
whoever asked for the action. Permission hardening is reused from `paths::secure_private_file`, so
mode `0600` on unix and the hardened DACL on Windows have exactly one home rather than two.

The store lives under the profile's **state** directory. State rather than cache or runtime is a
consequence, not a preference: a cache may be cleared by the OS at any moment, and a runtime directory is
login-lifetime on Linux. Either would silently make every pending approval undecidable across a logout or
a reboot — the exact restart case the acceptance test requires to work.

**2. The nonce is removed from disk *before* it is returned, which makes the one-time property a
filesystem property.**

`SecretStore::take` reads, removes, and only then parses and returns. Two concurrent decisions race on one
`remove_file` and exactly one observes the value; the loser sees `Absent`. This is stronger than an
in-memory flag because it holds across processes and across a restart, and its failure direction is the
safe one: a crash between the removal and the return loses the nonce, so the approval becomes
**undecidable** rather than **reusable**. A lost approval is recoverable by requesting the action again;
a reused one is a duplicated effect.

A corrupt stored value is consumed as well as reported, so a retry loop cannot repeat the read.

**3. `ApprovalRequest::nonce()` returns the typed `DecisionNonce`, never a `&str`.**

The nonce and its stored digest are **both** fixed-length lowercase hexadecimal, so a caller holding a
`&str` cannot distinguish them, and a digest passed where a nonce belongs would be written out as the
presentable secret — recreating precisely the flaw `P3-004` found, when a value fabricated from the
digest defeated the digest's purpose. Returning the type makes "the digest is not the nonce" a property
of the type system rather than of a reader's attention.

**4. The identifier is parsed, not sanitized, before it is joined to a path.**

`SecretStore::path_for` parses an `ApprovalId` and returns `InvalidId` for anything else, so
`../../escape`, an absolute path, and a bare `not-a-uuid` are refused *before* reaching the filesystem.
There is no escaping scheme to get wrong, because a malformed identifier cannot name a file.

## Consequences

- **A pending approval is now decidable, and the test proves it end to end**: hold a call, take the nonce
  as the operator's client would, and record a decision that moves the row to `approved`. This was
  impossible before this decision, and no unit test could have shown it — the gap was *between* a
  correctly-implemented store and a correctly-implemented pipeline.
- **The self-approval refusal is reachable and load-bearing.** Deciding as the run is refused, so the
  identity choice `P3-012a` made now has an executable consequence rather than a documented one.
- **A refused decision does not consume the nonce.** The domain refuses *after* the digest matches, and
  the guarded `UPDATE` rotates the digest only when the write lands, so an honest mistake (deciding as the
  wrong identity) does not burn an approval. Asserted in the test rather than reasoned about.
- **A write order is fixed and stated**: the approval row is written first and the secret second. A crash
  between them leaves an approval nothing can decide, which an operator can see and act on. The reverse
  order would leave a secret whose digest no row holds — a value nothing can consume and nothing will
  ever clean up.
- **A decided approval's file is discarded** (`SecretStore::discard`), so a presentable secret does not
  sit on disk for the remainder of an approval's lifetime after the decision has already been recorded.
- **This separates the agent from the human, not one human from another.** A local single-owner profile
  has one filesystem account. That is the boundary the threat model names; per-account separation for a
  multi-user deployment belongs to the server backend and its own slice.

## Honest limits

- **Nothing yet consumes the delivered nonce through a route.** The delivery, the one-time property, and
  the decision are proved at the pipeline and storage layers; a `POST /approvals/{id}/decision` route,
  and the resumption of the held call that follows a decision, are `P3-012b`'s remainder. Recording a
  decision today changes the approval's state but does not yet re-drive the admitted call to the adapter —
  and *that* step is where a duplicate delivery would become a second effect, which is why it is not
  rushed into this change.
- **The secret is written to disk in cleartext, protected by filesystem permissions only.** An OS keyring
  would be stronger, and `security.md` lists keyring-backed secret storage as the target for
  *long-lived* provider credentials. A nonce with a one-hour maximum lifetime (`P3-004`) is a poor fit for
  a keyring's persistence model, and `CredentialStore` — the daemon's own authentication secret — already
  makes the same tradeoff. Revisit if a keyring-backed `SecretStore` is added for connectors (`P5`).
- **The store is per-profile, not per-approval-supervised.** Nothing sweeps an expired approval's nonce
  file, so an approval that lapses without a decision leaves a small file behind. It is unreadable by
  anything but the account, and it becomes inert when the approval's own expiry passes, but a sweep is a
  legitimate follow-up and is recorded rather than implied.

## Alternatives considered

- **Keep the nonce only in daemon memory.** Rejected: an approval that survives a restart is what `A05`
  requires, and a restart would make every pending approval undecidable — the failure this decision
  exists to fix, reintroduced.
- **Return the nonce in the hold's response.** Rejected, and this is the important rejection: the hold's
  response is produced on the path a tool-call request travels, and once the executor routes
  model→tool (`P3-012c`) that response reaches the **run**. Putting the nonce there would look like a fix
  today and become a self-approval primitive the moment that path is wired, because the same response
  shape would be handed to the party the nonce excludes.
- **Derive the nonce from the approval id and a server secret.** Rejected by `ADR-0018` already, for a
  reason that still holds: the derived value stays valid until the secret rotates, so it is not
  one-time.
- **Store the nonce in the approval row and instruct readers not to select the column.** Rejected by
  `ADR-0018`: a durable row is exactly the thing that leaves the process in a backup, an export, or a
  restored profile, so "do not read it" is not a control.
- **Decide without the nonce, relying on approver identity, strength, expiry, and intent.** Rejected as
  the *default*, though `ApprovalRequest::apply_verified_decision` remains public because the store needs
  it after checking the digest. Dropping the nonce entirely would make the recorded control
  (`security.md`'s one-time nonce) into a documented intention, and `P3-004`'s whole design assumes it is
  presented.

## Related

- [ADR-0018: An approval is a decision record bound to a digest](0018-approvals-bind-to-a-digest-and-store-no-bearer-token.md)
- [ADR-0022: A cancellation request carries no version](0022-a-cancellation-carries-no-version.md) — the same
  reasoning about a guard that can only refuse valid work, applied to the requester identity.
