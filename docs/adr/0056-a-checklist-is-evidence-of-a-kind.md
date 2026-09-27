# ADR-0056: A checklist is evidence of a kind, and a scaffold refuses to invent what it cannot know

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P5-003` asks for a "connector quality checklist and scaffold generator modeled on manifest-driven
integration projects". Both halves already had a precedent in this repository, and neither was a program:

- the checklist is `docs/architecture/tools-and-connectors.md`'s **Connector Completion Gate** — ten bullets
  ending "A connector is not complete until it has: …";
- the scaffold precedent is `example/plugins/_template.py`, whose value is not its content but that it exists,
  so a new plugin starts from the right shape.

So the question was not "what does a connector need" but **"what form should the answer take"**. A Markdown
file states it once and is afterwards trusted, and `AGENTS.md`'s hardest-won rule is that a recorded claim is
read downstream as verified evidence with nothing distinguishing "checked" from "assumed". That rule, the
`P5-001`/`P5-002` history of implementations disagreeing with their own doc comments, and this repository's
four recorded instances of a **declared-but-unconstructed** item all point the same way: the checklist should
be something a program evaluates.

Four findings shaped the design.

1. **"Webhook signature/replay tests **if applicable**" is a fact about the connector, not a judgement.** The
   gate must derive applicability from the manifest's own `WebhookSupport` — a push connector is not complete
   without those tests and a polling one cannot have them.
2. **A checklist whose items all read the same is a formality.** Several of the document's bullets are already
   inside the manifest; several are the author's word. Presenting them identically would let the strong ones be
   ignored at the same cost as the weak ones.
3. **A scaffold cannot know a provider's API.** Operations, auth methods, and documentation links are what the
   research record is *for*, so a scaffold that invented them would hand the author a plausible lie about a
   service nobody had read.
4. **Two defects the work found in itself, one of which the tests could not have caught.** The first version
   classified the two webhook items as evidence *derived from the manifest*, which conflated the applicability
   condition with the evidence and made the gate report a push connector complete with no signature test at
   all. The second was only visible by running the command: the sidecar's path was computed with
   `with_extension("readiness.json")`, which replaced only the last extension and produced
   `vendor.manifest.readiness.json` where the scaffold writes `vendor.readiness.json`, so the documented
   workflow silently produced a connector whose every declared item was a gap.

## Decision

### 1. Every checklist item is one of four evidence kinds, and the kind is reported

`ReadinessItem::evidence()` returns an `EvidenceStrength`, and `jarvis connector items` prints it beside each
item. The four kinds are the whole deliverable:

| Strength | Meaning | Count |
| --- | --- | --- |
| `Derived` | A fact inside the manifest, which could not be built without it | 3 |
| `DeclaredArtifact` | A repository-relative path naming an artifact | 3 |
| `DeclaredCount` | A number the author supplies | 5 |
| `Conditional` | An item that may be refused, and whose refusal must carry a reason | 1 |

The **counts are asserted in a test**, so moving an item between kinds has to be deliberate. A gate that
reported a uniform "ok" for all twelve would hide that nine of them rest on the author's word and three on the
manifest itself.

### 2. The two webhook items are declared test counts, NOT derived from the manifest

**This was the slice's central defect, and it is worth stating precisely.** The first version gave both items
`EvidenceStrength::Derived` on the reasoning that "the manifest declares push delivery". That conflates two
different questions:

- **does this connector need webhook tests** — a manifest fact, and the basis of `applies_to`;
- **does this connector have them** — not a manifest fact at all.

A manifest declaring `hmac_sha256` proves nothing was tested. The gate would have reported a push connector
**complete** with no signature or replay test, which is precisely the condition `WebhookRejection` exists to
make visible. The test that caught it was the one asserting a push connector *without* those suites still
reports them as gaps; it failed with `got []` for the two gaps it expected.

`applies_to` and `evidence` are therefore separate questions with separate answers, and a comment in
`evidence()` records why the obvious classification is wrong.

### 3. Applicability is derived from the manifest, and a wrong attestation is refused rather than dropped

`ReadinessItem::applies_to(&WebhookSupport)` is the condition, and it is checked in two places: the item is
omitted from `evaluate` and `gaps` when it does not apply, and an **attestation naming it is refused**
(`ReadinessError::NotApplicable`) rather than discarded. A silent drop would hide that the author is working
from a template rather than from their own connector.

### 4. A review exists only when the items are satisfied; gaps are what a caller asks before that

`ReadinessReview::evaluate` returns a value whose items are all satisfied, so "is this connector complete" is
answered by whether the value exists — the move `ADR-0037` made for an admitted request and `ADR-0055` made for
a consumable transaction. `gaps` is the complement and is an associated function, because a caller that already
has a review has nothing to learn from it. `gaps` does **not** refuse at the first missing item; it names every
outstanding item with its evidence kind, which is what a progress report wants.

### 5. Each declared kind has a shape, and the wrong shape is an error

- An artifact item **must** give a path and **must not** give a count.
- A suite item **must** give a count of at least one and **must not** give a path. `None` and `Some(0)` are
  different claims — "not reported yet" and "there are none" — and only the second is a refusal.
- Every item **must** state a purpose, because a checklist entry that cannot say what it is for is the
  formality this module exists to avoid.
- `covers_failure` is meaningful for **one** item, and a value on any other is refused rather than ignored: a
  caller that set it believed it said something, and silently discarding that belief is how a report claims
  coverage it does not have.
- The onboarding item must report failure coverage as present, because the document says "successful **and
  failed** onboarding tests" and a success-only suite is the common shortcut.

### 6. `EvidencePath` checks shape, not existence, and says so

It refuses an empty path, one with surrounding whitespace, an absolute path, a home-relative path, a `..`
segment, a colon anywhere, an unknown extension, and anything over 512 characters. It **cannot** prove a file
exists, because this crate has no filesystem and gaining one would put `std::fs` in an adapter whose value is
being testable as a function of its arguments.

The limitation is stated in the type's own documentation rather than left for a reader to discover. The honest
claim is "a path that could name a repository artifact", and it catches a pasted absolute path, a traversal,
and a value pointing at nothing in particular.

**A branch was removed from it, and the falsification run is what found it.** The first version had a separate
`value.chars().nth(1) == Some(':')` check for a Windows drive prefix, justified in a comment as "a colon is
legal later in a path on unix". That comment was **wrong**: the very next check refuses a colon anywhere, so
`C:/x` and `C:x` were both caught by it and the earlier branch could never fire. An unreachable refusal reads
as protection while enforcing nothing — the defect `P5-001` recorded for an unreachable bound, found here by
mutation rather than by reading.

### 7. The conditionally-refused item carries its reason in the type

`LiveSmokeTest` is `Present { path, gate }` or `Absent { reason }`, and **both** constructors refuse a blank
string. So "a connector without a live smoke test must say why" is enforced where the value is built rather
than by a check in the gate, and `evaluate` takes `Option<&LiveSmokeTest>` rather than an `Attestation`. The
`Conditional` item's evidence is therefore a different shape from the others, which is why it is a different
strength.

`ReadinessAssessment` carries the **strength** explicitly rather than letting a reader infer it from
`declared_by.is_some()`. The first version had no `evidence` field, so the conditional item — whose
`declared_by` is `None` because its evidence is a `LiveSmokeTest` — was indistinguishable from a derived one,
and an assertion counting `declared_by.is_none()` to find the strong half would have counted a stated *reason*
as a manifest fact.

### 8. The scaffold is generated from the manifest's own constructor, so it cannot be stale

A committed template drifts: `ConnectorManifest` gains a field, the template does not, and a new connector
starts from a manifest that fails its own validation far from the template. Building the skeleton in-process
makes that impossible.

### 9. The scaffold refuses to invent three things, and the skeleton is provably not a manifest

- **Operations are empty.** A scaffold cannot know what an API does, and a generated `list_messages` would
  declare effects and risk for an endpoint nobody checked. `ConnectorManifest::new` refuses an empty operation
  list, so the skeleton is **deliberately not yet valid**.
- **Auth methods, secret fields, and links are empty**, because all three are vendor facts.
- **The research date is required, not defaulted.** Defaulting it to today would put a real date in
  `last_verified` on a record whose every section is blank — a stub that looks verified, which is exactly what
  `external-research.md` exists to prevent. `jarvis connector new` refuses without `--research-date`, and the
  module doc records why the convenience would be a lie.

The skeleton carries every manifest field name plus `_comment` keys, and `ConnectorManifest`'s
`deny_unknown_fields` **refuses** it — so "the scaffold does not invent operations" is a checkable property.
The test asserts that refusal, every field name, the emptiness of the four collections, that every
non-field key starts with `_comment` and is non-trivial, and the classification's starting level
(`confidential`, because `ADR-0054` refuses anything below it for an outward-reaching operation, so an author
who adds one write operation does not meet that refusal as a surprise).

### 10. The scaffold reports what it did not answer, from the same list the gate uses

`Scaffold::outstanding_items()` is computed from `ALL_ITEMS`, so the scaffold and the gate cannot disagree
about what a connector needs. It supplies exactly one item — `OfficialSourcesDated`, because the research stub
carries the path and the date — and reports the other eleven.

### 11. The skeleton is built with a JSON writer, and the first version was not

The initial `manifest_json` was a `format!` template. Its explanatory comments contain JSON examples, so their
quotes needed escaping and were not — the template produced invalid JSON the moment a comment quoted a nested
document. `serde_json::json!` plus `to_string_pretty` puts the escaping in the writer's hands and makes the
comments ordinary strings. A hand-written template that must quote a document inside itself is a defect
waiting for an edit.

### 12. The two CLI verbs are local, and the reason is a property of the subject

`jarvis connector new` and `jarvis connector check` write and read **repository artifacts**: a manifest and a
research record that are reviewed in a pull request and versioned with the code. Putting them behind the daemon
would mean starting a service to write a file the service must read back to answer a question about a file.
Both work with no daemon running, as `jarvis doctor` does and for the same reason.

`check` exits `DoctorWarnings` (6) when items are outstanding, not a failure code: a connector that is not
finished yet is the normal state of work in progress, and a code that read as an error would fail a CI job for
a connector somebody is still writing.

The attestations live in a **sidecar** (`<connector>.readiness.json`) rather than in the manifest, because the
manifest's `deny_unknown_fields` means a readiness section would become part of the document an operator reads
to decide whether to grant access. Evidence about a connector's tests is not a fact about its authority.

### 13. The sidecar's name is computed in one place, and the first version got it wrong

`sidecar_path` strips a `.manifest` infix so `vendor.manifest.json` yields `vendor.readiness.json`, which is
what `connector new` writes. **The first version used `Path::with_extension("readiness.json")`, which replaces
only the last extension**, giving `vendor.manifest.readiness.json` — a path the scaffold never writes. The
documented workflow therefore produced a connector whose sidecar was not found and whose ten declared items all
silently became gaps.

**No unit test could have caught this**, because both sides were "correct" against the same wrong assumption;
the two disagreed only when the verbs were run in sequence. It is now a function with a regression test, so the
writer and the reader share one rule.

### 14. Every guard was falsified, and two of the run's findings were defects

All 21 guards, A-B-A (intact passes → neutered fails → restored passes). A′ restores with `WriteAllText` plus a
forward mtime bump, because `Copy-Item` restores the backup's own older timestamp and cargo then keeps running
the stale mutant — the trap `P5-001` recorded and `P5-002` hit.

The run produced three informative failures beyond the two defects above:

- **Two tests were insensitive to their mutation, and the fix was to strengthen the assertion, not the mutant.**
  `a_generated_manifest_skeleton_is_not_a_manifest` accepted *any* unknown-field refusal, so renaming a
  `_comment` key to `note` still failed to deserialize and the test passed. It now asserts that every
  non-field key starts with `_comment` and is longer than 40 characters, which is the property the mutation
  changes. The same test also now distinguishes a leaked `"extra"` key from an empty `operations` list, which
  one generic refusal could not.
- **A probe was written from a guess rather than read from the file.** The `NotApplicable` refusal is
  formatted across four lines and the scaffold's comment key is the JSON string `"_comment"`; both probes had
  to be read before they matched.
- **A harness bug produced a uniform false negative.** `cargo test -p a -p b <filter>` treats `<filter>` as
  another `-p` pattern, so the first run reported "vacuous" for all 21 cases because no test ran. Each case now
  names the one package that owns its test. **A falsification harness that runs nothing reports success-shaped
  output**, which is the trap the A-B-A design exists to expose and which it exposed here.

## Consequences

- `jarvis-connectors` gains `readiness` and `scaffold` (140 tests in the crate, up from 116);
  `apps/jarvis-cli` gains a `connector` verb group and depends on `jarvis-connectors`. The lock-file delta is
  one line — a dependency on a crate already in the workspace — so **no package joined the tree** and
  `cargo deny` is unchanged (all four sections ok).
- The completion gate is now executable: `jarvis connector check <manifest>` reads the manifest's own facts,
  folds in the author's sidecar, and reports every outstanding item with its evidence kind.
- `jarvis connector items` answers "what does finishing a connector involve" before anyone has a manifest,
  which the Markdown file could not do interactively.
- The scaffold's manifest is **deliberately invalid** and its own JSON says why, so the one artifact a
  generator produces cannot be mistaken for a finished declaration.
- `manifest::validate_compatibility` became `pub(crate)` so the scaffold's version check is the manifest's own
  rule rather than a second implementation, and `CompatibilityVerdict`, `ProviderIdempotency`,
  `ResidencyVerification`, `SecretKind`, and `SignatureEncoding` are now exported so a scaffold author and a
  manifest reader share one vocabulary.

## Limits

- **Nothing consumes the readiness review.** No connector exists, no daemon reads a sidecar, and no CI job runs
  `connector check`. `P5-004` onward is where a connector is written; wiring the gate into CI is follow-up work
  with no TODO item yet.
- **`EvidencePath` cannot prove a file exists.** The crate has no filesystem, so the check is shape-only. A
  connector can therefore pass the gate while naming three artifacts that do not exist. Recorded rather than
  glossed; the remedy is a caller that owns a filesystem, not a change here.
- **Nine of the twelve items rest on the author's word.** The gate narrows what can be asserted (a count of at
  least one, an explicit failure-coverage flag, a stated reason for a missing smoke test) but it cannot verify
  that the tests exist or that they test what they claim. That is the honest ceiling of a checklist without a
  build system.
- **The test counts are not compared against anything.** Nothing runs the suite and checks the number, so a
  connector can report twelve auth tests and have one. A count was chosen over a boolean because it is harder
  to satisfy by accident, not because it is verified.
- **No `--force` flag.** `jarvis connector new` refuses to overwrite and tells the caller to remove the file
  or change `--dir`. The usage string named a `--force` flag that did not exist, which is a help text promising
  a capability the program does not have; it was found while writing this ADR and **fixed rather than recorded**,
  because a one-line correction is cheaper than a documented inconsistency. The refusal itself stays: a
  generator that silently overwrote an author's edited manifest would be the worst thing a scaffold could do.
- **The scaffold writes two files and nothing else.** No source module, no test scaffolding, no `Cargo.toml`
  entry, and no registration in a connector list. A "scaffold" in the fuller sense would create a crate; this
  one creates the two documents whose shape is actually known, which is the part a template can get right.
- **The research stub's sections are questions, not answers.** That is deliberate, but it means the scaffold's
  value is bounded by how much a blank template can help — an author who does not read
  `external-research.md` gets a list of headings.
- **`jarvis connector check` reads one manifest at a time** and has no notion of a connector *repository*, so
  it cannot report on a whole catalogue. `P5-004` onward builds one connector at a time, which is why one was
  enough.
