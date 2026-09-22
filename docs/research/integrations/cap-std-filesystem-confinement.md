---
integration: cap-std-filesystem-confinement
status: implemented
last_verified: 2026-09-22
owners: []
selected_spec_version: n/a (capability-based filesystem API)
selected_sdk: cap-std 4.0.3
---

# Confining The Read-Only Filesystem Tool With `cap-std`

## Scope

In scope: how the read-only filesystem tool (`P3-006`) is confined to explicitly granted workspace
roots, on Windows and unix, in a crate that **forbids `unsafe_code`**.

Out of scope: write, move, delete, and trash/undo (a later slice, and each needs its own undo design);
archive handling; and OS-level sandboxing of a child process, which is `P3-011`.

## The Requirement This Satisfies

`docs/architecture/tools-and-connectors.md` §High-Risk Adapters / Filesystem:

> Use explicit granted roots, handle-based resolution where possible, canonicalization plus
> race-resistant open semantics, symlink/junction policy, byte/file-count bounds, and trash/undo where
> supported.

`docs/architecture/security.md` lists it again as a threat to control and as a required test suite:

> | Filesystem escape | granted roots; handle-based/race-resistant access; symlink/junction policy; path
> normalization; size/count limits |
>
> "path traversal, symlink/junction race, archive, and filename tests on every OS"

Two of those clauses decide the entire design: **handle-based resolution** and **race-resistant open
semantics**. "Path normalization" is listed as the *weakest* of the controls, and treating it as the
primary one is the mistake this record exists to avoid.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| crates.io API | `https://crates.io/api/v1/crates/cap-std` | 2026-09-22 | version, license, features |
| `cargo info cap-std` | cap-std 4.0.3 | 2026-09-22 | resolved license and feature list |
| vendored source | `~/.cargo/registry/src/index.crates.io-*/cap-std-4.0.3/` | 2026-09-22 | the exact `Dir`/`File` API surface used |
| vendored backend | `.../cap-primitives-4.0.3/` | 2026-09-22 | confirms which syscall layer is underneath |
| `rustix` source | `.../rustix-1.1.5/src/fs/openat2.rs` | 2026-09-22 | confirms `openat2` is available to the backend |
| repository decision | `cap-std` README / `docs.rs/cap-std/4.0.3` | 2026-09-22 | the library's own statement of what it guarantees |
| locked graph | `Cargo.lock` | 2026-09-22 | confirms the resolved versions |

The API surface was read from the **vendored source at the version `Cargo.lock` resolves**, not from a
documentation page. That is the method `docs/development/external-research.md` ranks as "official SDK
source", and this project has already been bitten once by reading a version that was not the resolved
one (`P3-002`: `referencing` 0.33.0 and 0.57.0 have different constructors).

## The Decisive Constraint, Found Before Designing

`Cargo.toml` sets `[workspace.lints.rust] unsafe_code = "forbid"`, and `forbid` **cannot be overridden
by an inner `#[allow]`**. So a hand-written `openat`/`NtCreateFile` wrapper — the direct way to write
handle-based confinement — is not merely discouraged here, it does not compile. A safe crate has to
supply the primitive, and that narrowed the choice to two candidates.

| Candidate | Verdict |
| --- | --- |
| `rustix` 1.1.5 (already in the graph via `sqlx`) | `openat` + `openat2` with `RESOLVE_BENEATH` gives real kernel-enforced confinement, but it is **unix-only**, so Windows would still need its own implementation. Rejected as the primary mechanism, kept as the backend. |
| `cap-std` 4.0.3 | Capability-based `std`. Wraps `rustix` on unix and a safe `windows-sys` wrapper on Windows, and **does not expose `unsafe`**. Accepted. |

## The Defect In The Obvious Implementation

The obvious approach is: join the caller's path onto a root, canonicalize it, and check the canonical
path still begins with the root.

It is wrong, and the reason is that the check and the use happen at **two different moments**:

```text
1. check   canonicalize(path) -> "/granted/root/notes/todo.txt"   starts_with(root) -> true
2. swap    <-- the attacker replaces `notes` with a link to their own directory -->
3. open    open(path)         -> follows the link, reads the attacker's file
```

No amount of careful ordering removes it, because the check and the use are separate operations on a
name that can change in between. This is the "symlink/junction race" the security document requires a
test for, and a validation-based implementation cannot pass a real one.

### A claim in this record that was WRONG, and how it was corrected

The first version of the falsification test asserted something stronger and simpler: that a
**pre-existing** link inside the root defeats the prefix check. It **failed**, and the failure is worth
recording because assuming it would have left a wrong argument standing:

> On Windows, `fs::canonicalize` **resolves a junction**. So `canonical.starts_with(root)` is `false`
> for a link that already exists, and the naive check correctly refuses it.

So a pre-existing link is *not* the naive scheme's defect — a point-in-time check handles that case.
The defect is narrower and still fatal: the answer to "is this inside the root?" is only valid at the
instant it is computed, and the open happens later. The test now performs the naive scheme's own two
steps with the attacker's swap between them, which demonstrates the race deterministically and needs no
timing luck. **The general lesson: an argument for a design is not verified until the falsifying test
has been run, because a plausible defect can be the wrong defect, and a wrong argument that never fails
looks exactly like a right one.**

## Verified Contract

### Versions And Features

- **cap-std 4.0.3**, default features (there are none), so nothing is enabled implicitly.
- **Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT** — all three already in `deny.toml`'s allow
  list, so this adds no license to the policy.
- Backend resolved in this graph: `cap-primitives` **4.0.3**, `rustix` **1.1.5** (already present).
- The API surface actually used is small and stable: `Dir::open_ambient_dir`,
  `Dir::{open, read_dir, metadata}`, `File: Read`, `Metadata: len`, `ReadDir: Iterator<Item =
  io::Result<DirEntry>>`.

### What Is Relied On

1. **`Dir::open_ambient_dir` is the only ambient-authority entry point used.** It is called once per
   granted root, at construction. Everything after that resolves through the handle.
2. **`Dir::open` resolves each component beneath the directory handle**, so a resolution that would
   leave the root fails at the kernel boundary. On unix the backend can use `openat2` with
   `RESOLVE_BENEATH`; on Windows it resolves beneath the directory handle. The consequence relied on is
   the same on both: an escape is an error, not a file.
3. **An absolute path is not special.** `Dir::open("/etc/passwd")` is a handle-relative resolution, so
   the leading separator is an ordinary character and the host's `/etc` cannot be addressed. **Proven
   by running**, not read off a page — see the tests below.
4. **A junction or symlink inside the root pointing outside it is not "detected"** — it is followed and
   the escape fails at the boundary. The distinction matters because a detection is a check that can be
   bypassed while a boundary cannot.
5. **Not relied on: that a prefix check fails on a link.** It was measured and it does **not** —
   `fs::canonicalize` resolves a junction on Windows, so a point-in-time prefix check refuses a
   pre-existing link. This is exactly why the accepted design does not rest on a check; see "A claim in
   this record that was WRONG" below.

### Limits And Unknowns

- **`Dir` is not `clone`-able in a useful sense and the handle is not a path.** No accessor on
  `WorkspaceRoots` hands out a root as a plain `std::path::Path` for reading, deliberately: a caller
  that got one could pass it to `std::fs` and leave the boundary without noticing.
- **`cap-std` does not bound what you read.** A directory with a million entries or a 4 GB file is
  still your problem; `MAX_LISTED_ENTRIES` and a limited reader are this adapter's, not the crate's.
- **The crate's Windows backend is not independently audited here.** What is verified is the observed
  behaviour of a junction escape on Windows, which is the threat that matters; any stronger claim about
  the syscalls underneath is not made.
- **`Dir::open_ambient_dir` itself has no confinement** — it is the grant. That is why it appears
  exactly once, in `WorkspaceRoots::new`, and nowhere else in the crate.

## JARVIS Mapping

- `jarvis_tools::WorkspaceRoots` (`crates/jarvis-tools/src/workspace.rs`) owns the grant. It refuses an
  empty grant, a relative root, a duplicate root, and a root that is missing or not a directory —
  each a configuration fault, because a root is granted when a workspace opens rather than chosen by a
  caller. A missing root is an error rather than a skip: skipping it makes a workspace whose disk was
  not mounted read as "no files", which is indistinguishable from a working tool that found nothing.
- `jarvis_tools::FilesystemReadTool` (`crates/jarvis-tools/src/files.rs`) is the adapter:
  `jarvis.files.read` and `jarvis.files.list`, both `read_only`, risk 0, `Auto` approval, idempotency
  `Required`.
- Bounds: `MAX_TOOL_OUTPUT_BYTES` (32 KiB, applied at the read **and** by `BoundedOutput`) and
  `MAX_LISTED_ENTRIES` (1000, applied at the source). A listing is not bounded by the output bound until
  it is serialized, so a million-entry directory would otherwise build a million-element vector.

## Falsifying Tests

All in `crates/jarvis-tools/src/workspace.rs` and `crates/jarvis-tools/src/files.rs`, all passing.

| Property | Test | Falsified by |
| --- | --- | --- |
| **the naive check-and-open loses the race** | `the_naive_check_and_open_loses_the_race_that_the_handle_does_not` | — this is the demonstration that the design is load-bearing: it runs the rejected scheme and the accepted one on the same final state |
| traversal out of the root | `a_path_that_climbs_out_of_the_root_is_refused` | replacing the handle with a joined-and-checked path |
| an absolute path cannot reach the host | `an_absolute_path_cannot_address_the_host_filesystem` | resolving the caller's string against the process's ambient view |
| **a link out of the root cannot be traversed** | `a_directory_link_out_of_the_root_cannot_be_traversed` | any canonicalize-then-prefix-check implementation, for a link created after the check |
| the same escape through the whole adapter | `a_link_out_of_the_root_cannot_be_read_through_the_tool` | a bug in the adapter's path handling |
| oversized output | `an_oversized_file_is_truncated_and_marked` | reading the whole file and cutting afterwards |
| the bound boundary | `a_file_of_exactly_the_bound_is_not_truncated` | always reporting truncation |
| entry-count bound | (listing test) | listing without a cap |
| a binary file | `a_binary_file_is_reported_rather_than_lossily_decoded` | lossy decoding, which reports a mangled read as a success |

The link test carries an **ambient-authority control**: the same path is read with plain `std::fs` and
**succeeds**, proving the escape is real before asserting that the handle refuses it. Without that
control the test would pass on a mistyped fixture or a link that was never created — the "fixture
shares the code's assumptions" failure mode.

## A Platform Fact That Changed The Test

On this Windows machine, `New-Item -ItemType SymbolicLink` **fails** (no `SeCreateSymbolicLinkPrivilege`
and Developer Mode off), while `mklink /J` **succeeds**. Both were measured rather than assumed:

```text
SYMLINK_OK=False JUNCTION_OK=True
```

That matters beyond the test. **An unprivileged user can create a junction but often cannot create a
symlink**, so on Windows the junction is the escape vector that actually exists. A test that only
created symlinks would exercise a primitive most attackers cannot use. The link helper therefore
creates a **junction on Windows and a symlink on unix**, and panics rather than skipping if the
creation fails, because a skipped link test passes while proving nothing.

## Two Defects Found By Running, Not By Review

Neither was visible in the code as written; both were found by the first execution of the adapter's
tests.

1. **A binary file was reported as a successful read of nothing.** `String::from_utf8` reports the
   valid prefix, and a file that is invalid from its first byte has a valid prefix of **zero** — which
   decodes to an empty `String`, which *is* valid UTF-8. So a `.bin` file produced
   `Confirmed` with no content. Fixed by refusing when `valid_up_to() == 0`: a file that does not begin
   with UTF-8 text is reported, not returned as silent emptiness.
2. **The truncation flag was discarded, so a cut read was recorded as complete.** The reader stopped one
   byte past its limit (correct, and the only way to avoid allocating a 4 GB file), then handed the
   already-limited text to `BoundedOutput::truncating`, which decides truncation **by length** — and a
   value of exactly the limit reads as "fits". The fix is a new constructor,
   `BoundedOutput::from_bounded(content, truncated)`, because a reader that bounded at the source holds
   a fact the type cannot infer. It still bounds the content, so it cannot be used to smuggle oversized
   output past the bound; it can only carry a claim the caller has evidence for.

   **The general lesson:** when a type infers a property from a value's *shape*, a caller that knows the
   property directly must be able to state it, or the inference silently overrules the knowledge.

3. A third, smaller one: the test fixture's timestamps were anchored to a hardcoded calendar instant,
   and the adapter's deadline check reads the **real clock** — so once that instant passed, every read
   failed with a deadline refusal instead of the thing under test. Fixtures now use a relative offset
   from `UtcTimestamp::now`, which cannot drift.
