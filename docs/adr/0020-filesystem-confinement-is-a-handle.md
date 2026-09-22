# ADR-0020: Filesystem confinement is a directory handle, not a validated path

**Status:** Accepted

**Date:** 2026-09-22

## Context

`P3-006` requires "a read-only filesystem tool constrained to explicit workspace roots; test
traversal, links, races, and oversized output". It is the first real `ToolExecutor` adapter, and
therefore the first consumer of everything `P3-001` through `P3-005` built.

Three documents constrain it, and the first is the one that decides the design.

`docs/architecture/tools-and-connectors.md` §High-Risk Adapters / Filesystem requires "explicit granted
roots, **handle-based resolution where possible**, canonicalization plus **race-resistant open
semantics**, symlink/junction policy, byte/file-count bounds, and trash/undo where supported."

`docs/architecture/security.md` repeats it in the threat table — "Filesystem escape | granted roots;
handle-based/race-resistant access; symlink/junction policy; path normalization; size/count limits" —
and lists "path traversal, symlink/junction race, archive, and filename tests on every OS" as a
required test suite.

`Cargo.toml` sets `[workspace.lints.rust] unsafe_code = "forbid"`, and `forbid` **cannot** be
overridden by an inner `#[allow]`. So the direct way to write handle-based confinement — a
`openat`/`NtCreateFile` wrapper — does not compile in this workspace. A safe crate has to supply the
primitive.

Two decisions were needed: **what the confinement mechanism is**, and **what the adapter is allowed to
read**.

## Decision

**1. Confinement is a `cap_std::fs::Dir` handle per granted root, not a validated path.** The obvious
implementation — join the caller's path onto a root, canonicalize, and check the prefix still matches —
has a window between the **check and the use**: the canonical answer is valid only at the instant it is
computed, and the open happens later, so a component swapped in between is followed by the open. That is
the "symlink/junction race" the security document requires a test for, and no careful ordering removes
it, because the two are separate operations on a name that can change in between. A handle resolves each
component beneath itself at the open, so an escape is an error rather than a file.

`docs/architecture/security.md` lists "path normalization" among the controls but alongside
"handle-based/race-resistant access", and this record reads that ordering as intentional: normalization
is the weakest of the three.

**A falsification run corrected this argument.** The first version of the falsifying test asserted that a
**pre-existing** link defeats a prefix check. It failed, because on Windows `fs::canonicalize` *resolves*
a junction, so a point-in-time check refuses a pre-existing link correctly. The naive scheme's defect is
narrower and still fatal: its answer is only true when it is computed. The test now performs the naive
scheme's own two steps with a swap between them, which demonstrates the race deterministically. The
correction is recorded because a wrong argument that never fails looks exactly like a right one.

**2. `cap-std` 4.0.3 supplies the handle, because `unsafe_code` is forbidden.** `rustix` — already in
the graph via `sqlx` — offers `openat2` with `RESOLVE_BENEATH`, which is real kernel-enforced
confinement, but it is unix-only, so Windows would still need its own implementation. `cap-std` wraps
`rustix` on unix and a safe `windows-sys` wrapper on Windows, and does not expose `unsafe`. Licenses
(`Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT`) were already in `deny.toml`'s allow list.

**3. The ambient-authority entry point appears exactly once.** `Dir::open_ambient_dir` is the grant and
has no confinement of its own, so it is called in `WorkspaceRoots::new` and nowhere else. Every later
read goes through the handle. The type is not `Clone`, and no accessor hands out a root as a plain
`std::path::Path` for reading, so a caller cannot pass one to `std::fs` and leave the boundary without
noticing.

**4. An unusable grant is refused, never narrowed.** An empty grant, a relative root, a duplicate root,
and a root that is missing or is not a directory are all errors. A missing root especially: skipping it
would make a workspace whose disk was not mounted read as a workspace with fewer files, which is
indistinguishable from a working tool that found nothing. A relative root is refused because "relative
to what" has no safe answer — the daemon's working directory is not a workspace, and a service manager
may start it anywhere.

**5. A refused path is a `Failed` outcome, not an `AdapterError::RefusedBeforeReaching`.** The
distinction is the honest-outcome design of `P3-005` applied to this adapter. A refusal to serve a path
**has reached the filesystem** — a resolution was attempted and failed — so "nothing happened at all"
would be false. `Failed` says the call ran and established that no effect occurred and nothing was
read. `RefusedBeforeReaching` is reserved for the cases where nothing was touched at all: a malformed
argument, or a call already past its deadline.

**6. The output bound is applied at the read, not only by `BoundedOutput`.** The file is read through a
limited reader that takes one byte past the bound, and stops. Reading it into memory and then bounding
it would turn a 4 GB file into an out-of-memory instead of a truncated answer. One byte past the bound
is read so that "exactly the bound" and "more than the bound" are distinguishable — the difference the
truncation flag reports.

**7. `BoundedOutput::from_bounded` exists because a source-bounded reader holds a fact the type cannot
infer.** `truncating` decides by length, and a reader that stopped at the limit hands it a value of
exactly the limit, which reads as "fits". So a truncated read was recorded as complete — a silent lie
about a file the model would then treat as whole. The new constructor carries the caller's claim, and
still bounds the content, so it cannot be used to smuggle oversized output past the bound; it can only
carry a claim the caller has evidence for.

**8. A file that is not valid UTF-8 text is `Failed`, not an empty success.** Found by running: a file
invalid from its first byte has a valid UTF-8 prefix of **zero**, which decodes to an empty `String`,
which *is* valid — so a binary file produced `Confirmed` with no content. The prefix is refused when it
is empty, and the content is never lossily converted, because replacing bytes with U+FFFD would make a
mangled read look like a successful one.

**9. The tools are read-only, risk 0, `Auto`, idempotency `Required`.** `jarvis.files.read` and
`jarvis.files.list`, both `read_only`. The guidance table in `docs/architecture/tools-and-connectors.md`
puts risk 0 at "auto when scoped", and a read discloses rather than changes, so no human decides and a
repeat is meaningful. **The write half is deliberately not built**: a write needs an undo design this
slice does not have, and the read half's failure mode is disclosure — which confinement prevents —
rather than destruction.

## Consequences

- An escape cannot be expressed as a path, so there is no check to bypass and no race to lose. The
  tests assert the behaviour of a *real* junction on Windows and a symlink on unix, with an
  ambient-authority control proving the escape is real before asserting it is refused, and a separate
  test that runs the **rejected** scheme beside the accepted one so the design's value is demonstrated
  rather than asserted.
- Windows and unix use one code path, so the platform-specific escape vectors are the crate's problem
  rather than this adapter's — which is the reason to take the dependency at all.
- The adapter's `execute` is total for a refusal: an I/O error becomes a `Failed` result with a bounded
  reason, so a caller never sees an adapter error for something that was reached.
- The reason a refusal carries names the **kind** (`not found`, `not permitted`) rather than being
  flattened to "denied", because a missing file and a resolution that left the root arrive as different
  kinds and collapsing them hides the difference between a typo and an escape attempt.
- `jarvis-tools` gained its first adapter, which is what makes the crate the "execution pipeline" crate
  `repository-layout.md` names rather than only a contract crate.

## Honest limits at the time of this decision

- **No entry point drives the tool.** `FilesystemReadTool` implements `ToolExecutor` and is exercised by
  tests only. Nothing in `apps/jarvisd` registers the two definitions, evaluates policy, requests an
  approval, or calls `execute`, so no model can read a file yet. `P3-012` is where the Phase 3 gate
  proves the path end to end.
- **No registry wiring.** `FilesystemReadTool::definitions()` returns the two canonical definitions, but
  nothing inserts them into a `ToolRegistry` in production code — the composition step that joins the
  registry, the policy engine, the approval repository, and this adapter is not written.
- **Read-only, with no write, move, delete, or trash.** Trash/undo, which the architecture lists, is a
  separate design.
- **The deadline is checked, not enforced.** The adapter reads the clock before starting and refuses a
  lapsed call, but a read that begins in time and then blocks on a slow network mount is not
  interrupted; that needs cancellation, which is `P3-011`.
- **Blocking I/O on the calling thread.** `tokio`'s `fs` feature is not enabled for this crate, and a
  granted local read is not the long operation a blocking pool exists for — but a slow mount would block
  a worker, and resource limits for that belong to `P3-011`.
- **No archive or filename edge cases** beyond the path checks: a very long filename, a name that is not
  valid UTF-8 (reported as `<non-utf8 name>` in a listing rather than skipped), and NTFS alternate data
  streams are not separately tested.
- **The `cap-std` Windows backend is not independently audited here.** What is verified is the observed
  refusal of a junction escape on Windows, which is the threat that matters; no stronger claim about the
  syscalls underneath is made.

## Alternatives considered

- **Canonicalize the joined path and check the prefix.** Rejected: a TOCTOU window between the check and
  the open, which is the race the requirement names. It also "detects" rather than prevents, and a
  detection is a check that can be bypassed.
- **Hand-written `openat`/`NtCreateFile` FFI.** Not available: `unsafe_code = "forbid"`.
- **`rustix` with `openat2` and `RESOLVE_BENEATH` alone.** Rejected as the primary mechanism because it
  is unix-only, so Windows — the platform this project is developed on — would need a second
  implementation and a second set of tests. It remains the unix backend inside `cap-std`.
- **A path-prefix check on the canonicalized root, with links refused by `symlink_metadata`.**
  Rejected: it refuses links inside the root that point *within* the root as well, so a workspace using
  links legitimately stops working, and it still has the window for a link created after the check.
- **Reopen the root on every call instead of holding a handle.** Rejected: the grant is a decision made
  once, and re-opening turns a boundary into a repeated resolution that can fail or be redirected.
- **Treat a missing root as an empty workspace.** Rejected: it makes an unmounted disk read as an empty
  directory, which is indistinguishable from a tool that found nothing.
- **Return the whole file and bound it afterwards.** Rejected: a 4 GB file becomes an out-of-memory
  rather than a truncated answer.
- **Lossily decode what is not valid UTF-8.** Rejected: U+FFFD makes a mangled read look successful,
  which is the "success-sounding string" the architecture forbids.
- **Put the tool in a new `jarvis-connectors` crate.** Rejected: a local filesystem primitive is not a
  service connector with OAuth and accounts, and `repository-layout.md` gives `jarvis-tools` the
  execution pipeline and sandbox adapters.
- **Report a refused path as `AdapterError::RefusedBeforeReaching`.** Rejected as dishonest: the
  filesystem was reached and refused.

## Conditions that would justify revisiting

- A write, move, or delete tool arrives, which needs an undo/trash design and a different risk and
  approval posture.
- `P3-012`'s gate shows that a composition root needs something from the adapter that its current shape
  cannot express.
- A slow or network mount makes the blocking read a measured problem, which would move it behind a
  blocking pool and connect it to `P3-011`'s cancellation.
- A second adapter needs the same root confinement, which would make `WorkspaceRoots` a shared
  composition input rather than the filesystem adapter's own.
- The `cap-std` dependency proves unmaintained or its Windows behaviour changes, which would make the
  `openat2`-plus-Windows-backend split worth the second implementation.
