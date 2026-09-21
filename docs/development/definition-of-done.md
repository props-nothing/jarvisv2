# Definition Of Done

A TODO item is done only when every applicable section passes. "Code exists," "it compiles," and "the model said it worked" are not completion criteria.

## Scope And Architecture

- [ ] The change maps to named `TODO.md` task IDs and stays within their acceptance boundary.
- [ ] Ownership matches `repository-layout.md`; provider/framework types do not leak inward.
- [ ] Existing public APIs and persisted behavior are preserved or intentionally versioned/migrated.
- [ ] A new/superseding ADR exists for a durable architecture or trust-boundary change.
- [ ] No placeholder, mock-only path, or uncalled library is described as complete.

## External Evidence

- [ ] Not applicable, or live official docs/spec/SDK/changelog were fetched during the task.
- [ ] The dated integration research record contains selected versions and relevant changes.
- [ ] Auth, scopes, secrets, limits, pagination, idempotency, retries, webhooks, privacy, deprecations, and ambiguity are addressed.
- [ ] SDK/dependency license, features, advisories, and removal/degradation plan are understood.

## Behavior

- [ ] The behavior is reachable through a real application/client path.
- [ ] Success, empty, denied, invalid, unavailable, timeout, cancellation, retry, and ambiguous outcomes are handled as applicable.
- [ ] State transitions and emitted events are durable/ordered where required.
- [ ] External effects have explicit idempotency or non-retryable/unknown semantics.
- [ ] User-visible wording distinguishes intent, submission, confirmation, failure, and uncertainty.

## Security And Privacy

- [ ] Authentication and workspace/resource authorization are enforced at the final boundary.
- [ ] The model cannot approve or broaden its own authority.
- [ ] Secrets use `SecretRef`, stay out of prompts/logs/errors/URLs/fixtures, and are redaction-tested.
- [ ] Untrusted input, prompt injection, SSRF, paths, redirects, payload sizes, and output bounds are addressed as relevant.
- [ ] Sensitive data classification, retention, export, and deletion behavior are documented and tested.
- [ ] A falsification/negative test proves the guard fails closed.

## Data And Compatibility

- [ ] Migrations are additive/reversible or have a documented backup/rollback path.
- [ ] Empty install and supported upgrade paths pass on applicable backends.
- [ ] Concurrency, uniqueness, workspace filtering, pagination, and deletion invariants pass.
- [ ] Configuration/protocol/schema versions and compatibility windows are updated.
- [ ] Backup/restore implications are addressed.

## Tests

- [ ] The cheapest behavior-scoped test passes immediately after the change.
- [ ] Unit and adapter/repository contract tests pass.
- [ ] Integration, platform, live-provider, and end-to-end tests pass where ownership requires them.
- [ ] Fixtures are sanitized, sourced, versioned, and include failure shapes.
- [ ] Formatting, linting, workspace tests, security/dependency/license checks pass.
- [ ] Flaky timing, real clock, global state, or credential-dependent default tests were not introduced.

## Operations And UX

- [ ] Health, `status`, `doctor`, logs, metrics/traces, and redacted diagnostics expose the new failure mode as appropriate.
- [ ] Timeouts, cancellation, resource cleanup, retries, backpressure, and shutdown are tested.
- [ ] CLI/API/UI behavior is consistent and accessible where multiple clients expose it.
- [ ] Install, update, rollback, and uninstall implications are handled.
- [ ] Cost/quota/concurrency impact and operator/user controls are visible.

## Documentation And Closeout

- [ ] Canonical docs and generated API/schema docs are updated without contradictory copies.
- [ ] Limitations and unresolved questions are explicit.
- [ ] `TODO.md` is checked only after evidence passes; follow-up work is separately tracked.
- [ ] Commands actually run and results are reported.
- [ ] No unrelated changes, credentials, personal data, build output, or local state are included.

## Phase Gate

A roadmap phase completes only when all phase TODOs pass this definition and the corresponding scenarios in [acceptance-tests.md](../quality/acceptance-tests.md) are proven on the required platforms. Research completion is not implementation completion; implementation completion is not live verification unless the phase requires it.