---
integration: github-actions-rust-supply-chain
status: implemented
last_verified: 2026-09-20
owners: []
selected_spec_version: GitHub Actions current cloud workflow syntax
selected_sdk: cargo-deny-action 2.1.1 / cargo-deny 0.20.2
---

# GitHub Actions Rust Supply Chain

## Scope

This record covers Phase 1 CI on GitHub-hosted Windows, macOS, and Linux runners plus Rust dependency advisory, license, source, and duplicate-version policy. Release publishing, signing, deployment, self-hosted runners, and provider credentials are out of scope.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| GitHub Docs `llms.txt` | https://docs.github.com/llms.txt | 2026-09-20 | official documentation discovery |
| GitHub Actions workflow syntax | https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-syntax | 2026-09-20 | jobs, matrices, permissions, runners, action references |
| GitHub Actions secure use | https://docs.github.com/en/actions/security-for-github-actions/security-guides/security-hardening-for-github-actions | 2026-09-20 | least privilege and immutable action pinning |
| `actions/checkout` release | https://github.com/actions/checkout/releases/tag/v7.0.1 | 2026-09-20 | selected checkout release and verified commit |
| `cargo-deny` AI index | not found at https://embarkstudios.github.io/cargo-deny/llms.txt | 2026-09-20 | official mdBook pages used instead |
| `cargo-deny` documentation | https://embarkstudios.github.io/cargo-deny/ | 2026-09-20 | supported checks and CLI |
| License configuration | https://embarkstudios.github.io/cargo-deny/checks/licenses/cfg.html | 2026-09-20 | deny-by-default SPDX policy and private workspaces |
| Advisory configuration | https://embarkstudios.github.io/cargo-deny/checks/advisories/cfg.html | 2026-09-20 | RustSec database and advisory behavior |
| Source configuration | https://embarkstudios.github.io/cargo-deny/checks/sources/cfg.html | 2026-09-20 | registry and Git-source allowlist |
| Ban configuration | https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html | 2026-09-20 | wildcard and duplicate dependency policy |
| `cargo-deny-action` source/release | https://github.com/EmbarkStudios/cargo-deny-action/releases/tag/v2.1.1 | 2026-09-20 | action version, bundled `cargo-deny` 0.20.2, inputs |
| RustSec advisory database | https://github.com/rustsec/advisory-db | 2026-09-20 | advisory authority and update model |
| crates.io `cargo-deny` metadata | https://crates.io/api/v1/crates/cargo-deny | 2026-09-20 | 0.20.2 version, Rust 1.88 MSRV, MIT OR Apache-2.0 |

## Verified Contract

### Operations And Transport

GitHub workflow files live under `.github/workflows` and may use a matrix to run independent jobs on hosted OS images. Each job receives a fresh hosted runner. The Phase 1 workflow runs formatting, warning-denied Clippy, tests, and the native phase gate on each supported OS. A separate Ubuntu job runs `cargo-deny check advisories bans licenses sources` against the committed lockfile.

The phase gate is a real process-level test, not a script: `tests/e2e` is a
workspace member whose integration test (`phase1_gate`) builds and runs the
`jarvisd` and `jarvis` binaries against a temporary portable root on the native OS.
It asserts, in order: a clean profile, durable readiness, health and status over the
local transport, state surviving a restart, schema integrity, a healthy diagnosis,
and a deliberately broken configuration producing a named finding with a
remediation while remaining unmodified.

The gate is skipped with a message when the binaries are absent so plain
`cargo test` still works before a build; CI sets `ACCEPTANCE_REQUIRE_BINARIES=1` to
turn that skip into a failure. That variable deliberately avoids the `JARVIS_`
prefix, because the daemon rejects unknown `JARVIS_*` variables as configuration
errors and would refuse to start. This was observed: naming it `JARVIS_REQUIRE_ACCEPTANCE`
made `jarvisd` exit with `unknown configuration environment variable`.

The selected immutable action revisions are:

- `actions/checkout` v7.0.1 commit `3d3c42e5aac5ba805825da76410c181273ba90b1`
- `EmbarkStudios/cargo-deny-action` 2.1.1 commit `3c6349835b2b7b196a839186cb8b78e02f7b5f25`

### Authentication And Authorization

The workflow needs only repository content read access. It sets `permissions: contents: read`, has no deployment environment, receives no provider credentials, and does not use `pull_request_target`. GitHub documents full-length action commit SHAs as the only immutable action reference.

### Limits And Failure Semantics

Jobs have explicit timeouts and fail on non-zero command status. The OS matrix uses `fail-fast: false` so one platform failure does not erase evidence from the others. Advisory checks fetch the current RustSec database; vulnerabilities fail, yanked crates fail, and no advisory ignores are configured. CI does not retry a failed quality or policy gate automatically inside the job.

### Data And Compliance

Repository source and Cargo metadata are processed on ephemeral GitHub-hosted runners. `cargo-deny` contacts crates.io metadata and the public RustSec advisory database. No JARVIS runtime data, secrets, transcripts, or user content is present or required. GitHub-hosted CI cost and retention follow repository plan settings and are not controlled by JARVIS.

### Versions And Deprecations

`cargo-deny-action` 2.1.1 bundles stable `cargo-deny` 0.20.2 and fixed the removed `use-git-cli` argument from 2.1.0. `cargo-deny` 0.20.2 requires Rust 1.88, below the repository's pinned Rust 1.98.1. Action tags are documented for maintainers, but execution is pinned to the verified release commits.

## JARVIS Mapping

- Hosted runner OS labels provide native CI evidence, not runtime platform abstraction.
- `cargo-deny` advisory failures are supply-chain gate failures, not JARVIS domain errors.
- Dependency licenses are allowlisted independently of the unresolved JARVIS source license.
- Workspace crates are private (`publish = false`) and ignored by dependency-license scanning until the owner chooses the platform license.
- No CI identity or token crosses into JARVIS binaries, configuration, logs, or tests.

## Decisions

- Use GitHub-hosted runners for isolation and avoid self-hosted runners in Phase 1.
- Pin every external action to a full verified commit SHA and retain the release tag in a comment.
- Grant only `contents: read` to the workflow token.
- Run native format, Clippy, tests, and the Phase 1 gate on Windows, macOS, and Linux.
- Use one `cargo-deny` job for advisories, bans, licenses, and sources.
- Allow common permissive dependency licenses; deny all unlisted licenses and non-crates.io sources.
- Ignore unpublished workspace packages in the dependency-license check because the project license remains intentionally undecided.

## Rejected Alternatives

- A second `rustsec/audit-check` action was rejected because `cargo-deny` already consumes RustSec advisories; it would add another executable action without a distinct Phase 1 assertion.
- Floating action tags were rejected because GitHub identifies full SHAs as the immutable option.
- Self-hosted runners were rejected because GitHub warns that untrusted workflow code can persistently compromise them.
- Selecting a JARVIS platform license implicitly was rejected; that owner decision remains outside dependency intake.

## Verification Plan

- Parse the workflow and assert every external `uses:` reference ends in a 40-character SHA.
- Run the three baseline Cargo commands locally and on every CI OS.
- Run `cargo deny check advisories bans licenses sources` against the committed lockfile.
- Falsification: temporarily remove an encountered dependency license from the allowlist and prove the license job fails, then restore the policy.
- Falsification: point a fixture dependency at an unapproved Git source and prove the source check fails.
- Run the Phase 1 process-level gate on each native hosted runner. **Implemented in `P1-012`**: `cargo test -p jarvis-acceptance --test phase1_gate` after `cargo build --workspace`, with `ACCEPTANCE_REQUIRE_BINARIES=1` so a missing binary fails rather than skips.
- Falsification for the gate: point `--root` at a directory that cannot be prepared and prove the gate fails at the readiness step rather than passing vacuously.

The cheapest central-contract test is a local `cargo deny check advisories bans licenses sources`; it disproves malformed policy and rejected dependencies before CI.

## Unresolved Questions

- The JARVIS platform source license is not selected. This blocks public distribution and release claims, but it does not block enforcing third-party dependency licenses for private workspace packages.
- GitHub-hosted runner image labels evolve. The workflow intentionally uses stable `*-latest` OS families for Phase 1 and records the concrete runner image in CI evidence; release artifacts will pin a stricter matrix later.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-20 | GitHub Actions current, checkout 7.0.1, cargo-deny-action 2.1.1, cargo-deny 0.20.2 | SHA-pinned workflow added; local advisory, ban, license, and source checks passed | GitHub Copilot |