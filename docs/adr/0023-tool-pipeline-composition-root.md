# ADR-0023: The tool pipeline is composed in the daemon over the adapter's own definitions

**Status:** Accepted

**Date:** 2026-09-22

## Context

`P3-001` through `P3-006` built every layer a tool call needs: a registry with compiled schemas
(`jarvis-tools`), a pure policy engine with three outcomes (ADR-0017), a durable call lifecycle with an
idempotency ledger (ADR-0019), an authorization receipt derived from its decision (ADR-0021), a request
bound to that receipt (`P3-006b`), and a filesystem adapter confined by a directory handle (ADR-0020).

Nothing composed them. Each layer was tested against the layer below it and no test reached a tool
through a route. A pipeline that is constructible but has no production caller is a pipeline whose
**seams** are unverified — and the last three slices found a real defect in every seam they examined
(`P3-006a`, `P3-006b`, `P3-006c`). The absence of a caller was therefore the largest remaining source
of unknown defects, not a deferred convenience.

The question is where the composition belongs, and what it may read.

## Decision

**1. Composition happens in `apps/jarvisd`, not in `jarvis-application`.**
`docs/architecture/repository-layout.md` states that no arrow may point from core or application into a
provider adapter, and that composition happens only in binaries and test harnesses. The pipeline needs a
concrete `FilesystemReadTool` and a concrete `SqliteDatabase`, so it is composed in the binary.

**2. The pipeline registers the adapter's own `definitions()`.**
Not a copy of the schema and risk level. `P3-003` decides about a `ToolDefinition`; if the pipeline
declared its own, policy would decide about a tool *other than the one being run*, and the two could
disagree without anything detecting it. This is the same defect class as `P3-006a` (two values that must
agree, with nothing holding both) applied one level up. An adapter that cannot state its own contract
therefore cannot be registered at all: `FilesystemReadTool::definitions()` is fallible, and a failure
stops startup.

**3. The workspace and the actor are derived, never accepted.**
`POST /api/v1/tools/{tool}/calls` accepts exactly `run_id` and `arguments`. The **workspace comes from
the stored run's row**, and the scope comes from the actor constructor the daemon calls — never from the
request body. `docs/architecture/identity-and-workspaces.md` requires access to follow from
authentication rather than from a client-supplied identifier, and a request that could name its own
workspace or grant would let a client widen its own authority. `arguments` is passed through
`deny_unknown_fields`, so an attempt to supply a `workspace` field is a `422` rather than a field that is
quietly ignored — an ignored field reads as an accepted one.

**4. No roots means no pipeline, not an empty one.**
`daemon.tool_workspace_roots` with no entries composes `None`. Registering the adapter over zero roots
would make the daemon advertise a tool that fails every call, which a caller reads as a broken tool
rather than an absent capability; the route answers `404` with the configuration key that would enable
it.

**5. An unusable grant fails at startup, not on the first call.**
`WorkspaceRoots::new` is called in `start()`, for the same reason the executor model is resolved there
(ADR-0011's reasoning applied to tools). ADR-0020's fourth rule is that an unusable grant is **refused,
never narrowed**; a root that cannot be opened must stop the daemon rather than produce a workspace whose
files appear to be simply absent.

**6. The call id **is** the correlation id.**
The pipeline does not mint a second identifier. One request produces one correlation id, which becomes
the call row's key, the receipt id, and the idempotency binding's reference — so a log line, a database
row, and a receipt all name the same call without a join.

## Consequences

- The whole path is reachable for the first time: route → handler → registry → schema → policy → receipt
  → admit → authorize → submit → adapter → recorded outcome. Seven composition tests exercise it,
  including one per refusal path.
- **A held decision is not an error and does not execute.** `AwaitingApproval` returns `202` carrying
  `call_id` and `required_strength`, and the call row is left `requested` so an approval has something
  to bind to. Running it would be the confused-deputy shape `docs/architecture/security.md` refuses.
- **The receipt is built with `approval: None` even for a held call**, because the held path returns
  before the receipt is used for anything but the call row. Fabricating an approval citation would be
  inventing authority.
- The handler maps a refusal to `403` with a reason code rather than a `5xx`: a refusal is a correct
  answer to a request, and a client should not retry it.
- `ToolPipeline::call_tool` was 117 lines and was split into `validate`, `authorize_and_admit`, and
  `execute_and_record`. The split is not cosmetic: `authorize_and_admit` returns `Option<PreparedCall>`
  so that "held" is a value rather than an early return buried in a long function, and `PreparedCall`
  groups the values that must agree so a receipt from one call cannot be paired with a key from another.

## Limits, stated rather than implied

- **There is no approval round-trip.** A held call returns `call_id` and `required_strength`, and then
  nothing: no `ApprovalRequest` is persisted, no route lets a human decide, and nothing resumes the
  call. The ledger row is truthfully `requested`, and it stays that way. This is a gap, not a design.
- **No `run_events` row is written for a tool call.** The call row is the audit record; linking calls to
  the event log is `P3-012`.
- **`policy_version` is a label, not a verifiable version.** The handler passes the literal `"policy-1"`.
  Nothing checks it against a published policy revision, so a receipt citing it proves which string was
  supplied, not which rules were applied.
- **The authentication strength is asserted, not proven.** The handler passes `Credential` because a
  loopback credential was presented on this transport. No per-call verification of that credential
  happens at the call site.

## Alternatives considered and rejected

- **Compose the pipeline in `jarvis-application`.** Rejected: it would require an arrow from application
  into `jarvis-tools` and `jarvis-storage`, which `repository-layout.md` forbids.
- **Let the request carry the workspace and the scope.** Rejected: it makes the client the authority on
  its own permissions, which `identity-and-workspaces.md` forbids.
- **Register restated schemas in the daemon.** Rejected: two copies of a contract that must agree, with
  nothing holding both — the `P3-006a` defect reproduced one level up.
- **Compose an empty pipeline when no roots are configured.** Rejected: it advertises a capability that
  cannot succeed, and a caller cannot distinguish "no files here" from "this tool is not configured".
- **Fail on the first tool call when a root is unusable.** Rejected: ADR-0020 forbids narrowing a grant,
  and a late failure leaves a run that was accepted against a tool that cannot work.

## Revisit if

- Approvals acquire a round-trip, at which point the held path must persist an `ApprovalRequest` and the
  `approval: None` construction must be replaced by a real citation.
- A second adapter is registered, at which point the single-adapter `registry` field must become a
  collection and "which tools does this daemon offer" becomes a configuration question.
- Policy versions become checked rather than labelled, at which point the literal `"policy-1"` must be
  replaced by a resolved revision.
