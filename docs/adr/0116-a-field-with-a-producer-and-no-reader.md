# ADR-0116: A field with a producer and no reader, and the two values that had to agree

- **Status:** Accepted
- **Date:** 2026-10-01
- **Slice:** `P5-005` (the Google connector — the health state's missing scopes reaching the diagnostic that
  names them).
- **Relates to:** `ADR-0092` (a value with a producer and no reader — the shape this repeats one level down),
  `ADR-0021` (two values that must agree, with nothing making them — here the state's list and the caller's),
  `ADR-0113` (the same "a value the consumer needs" question for the channel lease), and `ADR-0044` (a stored
  value that was not the one the report needed).

## Context

`ConnectorHealth::NeedsReauth` carries the scopes a reauth must name:

```rust
NeedsReauth {
    reason: ReauthReason,
    /// The scopes that are missing, when the reason is a scope loss.
    missing_scopes: Vec<String>,
    signal: HealthSignal,
}
```

`crate::diagnostics::diagnostics_for` emits the `MissingScopes` finding — whose own doc calls it *"the one list
that is reported rather than counted, because a reauth prompt needs to name them"* — and it decides that from a
**separate argument**:

```rust
pub fn diagnostics_for(
    health: &ConnectorHealth,
    auth_state: AuthState,
    missing_scopes: &[String],   // the caller's list, not the state's
) -> Vec<DiagnosticFinding>
```

Two facts made that a defect rather than a redundancy:

1. **The state's field had a producer and no reader.** It is written by every construction — five call sites in
   the tests, and it is the only place a `ScopeLoss` reauth can record *which* scopes — and **nothing reads
   it**. There is no accessor, and `diagnostics_for` matched only `NeedsReauth { reason, .. }` (the `..`
   discarding the list). So a caller that built a `ScopeLoss` state carrying `["gmail.readonly"]` and passed an
   empty shortfall produced a report with **no** `MissingScopes` finding at all.
2. **The two lists had to agree, and nothing made them.** The state's list and the argument's list describe the
   same fact — the scopes a reauth prompt needs — and only one of them reached the report. A caller could build
   a `ScopeLoss` state with one list and pass a different one, and the report would name the caller's while the
   stored state said otherwise. That is `ADR-0021`'s "two values that must agree, with nothing making them".

The tell is the same one `ADR-0092` records: **a field is either read or it is a claim.** Here the field's own
doc asserted a consumer — *"the list a reauth prompt needs"* — and no consumer existed, so the assertion was
true of the value and false of the code.

## Decision

**1. `ConnectorHealth::missing_scopes()` is the reader.**

```rust
pub fn missing_scopes(&self) -> &[String] {
    match self {
        Self::NeedsReauth { missing_scopes, .. } => missing_scopes,
        Self::Connected { .. } | Self::Degraded { .. } | Self::Disconnected { .. }
        | Self::Unknown { .. } => &[],
    }
}
```

The accessor shape `reauth_reason()` already uses, so a caller branches on `is_empty` rather than matching the
enum — a caller that had to match every variant would break on a new one (the same rule the health state's own
"every state carries its signal" test records). The return is a **slice, not an `Option<&Vec>`**: an absent list
and an empty one call for the same action here, and an `Option` would make a caller unwrap a value whose absence
is not actionable.

**2. `diagnostics_for` reads both sources, and emits one finding.**

```rust
let state_scopes = health.missing_scopes();
if !missing_scopes.is_empty() || !state_scopes.is_empty() {
    findings.push(/* MissingScopes, Warning */);
}
```

**Both** are consulted, deliberately, because the two describe genuinely different situations and each can be
true without the other:

- a **shortfall without a reauth** — a partial consent the account still runs under (`AccountStatus::ScopeShortfall`
  permits calls) — is only ever in the caller's list;
- a **`ScopeLoss` reauth** records the scopes in the **state**, because that is the value that survives the
  probe and is what a persisted health record holds.

The union is the honest answer, and it closes the drop: the list the state carries can no longer be discarded by
a caller. The finding is emitted **once** regardless of how many sources name scopes, because it describes the
*condition* rather than each scope.

**3. A dangling doc reference was corrected in the same module.**

`health.rs`'s module doc linked `ConnectorHealth::is_stale_at` — the method is `is_fresh_at`. `cargo doc`
warnings are not denied, so nothing compiled it; the link now names the method that exists.

## Consequences

- **The scopes a reauth state records reach the report.** A caller that builds a `ScopeLoss` state no longer has
  to *also* pass the same list to get a `MissingScopes` finding — the state is enough, which is what makes the
  field's own doc true of the code.
- **The two values can no longer disagree.** The report reflects the state's list, so a stored health record and
  the diagnostic derived from it name the same scopes.
- **Two guards falsified A-B-A**, both compiling: (1) `missing_scopes()` returning an empty slice for
  `NeedsReauth` → **detected by both** the accessor test (`left: []`, `right: ["calendar.readonly",
  "gmail.readonly"]`) and the diagnostics test; (2) `diagnostics_for` reading only the caller's argument again →
  **detected** (`a ScopeLoss state must report its own missing scopes without the caller supplying them`).
- **A `let _ = missing_scopes;` was removed**, which was the tell that the argument's only use was a
  discard — the compiler-level version of "a parameter nothing decides anything with".

## Alternatives considered

- **Drop the field from `NeedsReauth` and keep only the caller's argument.** Rejected: the field is what a
  persisted health record carries, and a reauth state is exactly where "which scopes" belongs — the remedy for
  a `ScopeLoss` is *request only what was lost*, which a report that had to be re-supplied the list could not
  reach. Removing it would trade a value with no reader for a reader with no value.
- **Make `diagnostics_for` take the scopes from the state only, and drop the argument.** Rejected: a scope
  shortfall without a reauth (`AccountStatus::ScopeShortfall`) has no `ConnectorHealth` variant carrying it, so
  the caller's list is the only source there. The two are not the same fact and neither subsumes the other.
- **Return `Option<&Vec<String>>` from the accessor, with `None` for states that carry none.** Rejected: `None`
  and an empty list call for the same action here, so the `Option` would be a value the caller must unwrap to
  learn nothing — the same reasoning the module's own `signal()` uses when it returns a `&HealthSignal` for
  every variant rather than an `Option`.
- **Assert the two lists are equal at construction (fail if they disagree).** Rejected: they are allowed to
  differ and the union is correct — a `ScopeLoss` state may name the scopes the probe found while the caller
  names an additional shortfall the state did not record. Equality would refuse a state that is right.
- **Leave the field and only document that the caller must pass the list.** Rejected: that is the "a doc comment
  saying what a value is FOR is a claim about a consumer" trap `ADR-0110` records — the doc was making a claim
  (`the list a reauth prompt needs`) that no code honoured, and correcting the code is what makes it true.

## Conditions that would justify revisiting

- **A push handler or `doctor` renders the report**, at which point the `MissingScopes` finding is presented to a
  user and the union's contents become observable rather than only asserted.
- **A `ConnectorHealth` variant for a shortfall without a reauth is added**, at which point the caller's argument
  becomes redundant and the accessor can be the single source — the "two sources" split would collapse into one.
- **A third source of missing scopes appears** (a required-scope set the manifest changed under an existing
  grant), at which point the union is the place the third joins rather than a fourth parameter.
