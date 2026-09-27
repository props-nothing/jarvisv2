# ADR-0046: Retrieval eligibility is a filter, and the ranking's explanation is its arithmetic

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P4-004` implements memory retrieval. `docs/architecture/memory-and-context.md` states it as two stages and
gives a list for each:

- **Eligibility** filters before ranking by actor authorization, workspace policy, status/validity/retention,
  sensitivity against the destination model, the memory type the use case allows, and a source trust
  minimum.
- **Ranking** combines "independently inspectable signals" with the requirement that "no single signal may
  dominate by accident", that component scores and the final inclusion reason be stored, and that results be
  diversified with per-source and per-category budgets.

Writing that produced five decisions worth recording, and three of them were a shape that looked obviously
right and was not.

## Decision

### 1. Eligibility is a filter, not a low score

Every one of the six rules is a predicate that **removes** a memory, rather than a signal that lowers its
rank. Three of them could not be ranking signals at all:

- A memory from another workspace is not a poorly-ranked result; it is not a result. Ranking it and dropping
  it afterwards would make the ranking depend on how many foreign memories happened to be in the candidate
  set, so one user's score would change because another user stored something.
- A superseded or expired claim is not "less relevant", it is not current truth, and the document requires
  it stop being retrieved **as current**. A ranker that could surface a superseded claim would need every
  consumer to re-check its currency before using it.
- A memory above the destination's ceiling must not be admitted and then redacted. A redaction is a
  transformation of content that has already entered the assembly step, and the document's rule is
  destination-facing: "sensitive memories can be excluded from remote models" is decided from the stored
  level, so a level above the ceiling is a disclosure no later check recovers.

The other three — type, trust, confidence — are requirements the **caller** states, and a filter that
ignored one would return something the caller had explicitly excluded. The second filter is where one of
the two gets forgotten, so there is only one.

The **order** is the document's, with one merge recorded below, and it is chosen so the reported reason is
the most specific: workspace, then destination, then currency, then the three caller-stated rules.

### 2. "Actor authorization" and "workspace policy" are one check here, and the reason is recorded

The document lists them as two rows. In this platform they are one, because actor authorization **is**
workspace membership: `P4-002`'s reads all bind `workspace_id` as their scope, and there is no second
workspace a local actor could be authorized for. A separate actor rule with nothing to distinguish would be
a rule that always passes — which is worse than an absent rule, because a reader would count it.

This is recorded rather than silently merged because the two *are* separate in a server deployment, where an
actor may hold grants across workspaces. `P4-009` owns the PostgreSQL backend and `P4-008` the sharing
surface; whoever implements either should split this check, and the merge is the thing to look for.

### 3. The weights are one table, capped at a quarter of the total each

"No single signal may dominate by accident" is a claim about numbers, so it is enforced as arithmetic rather
than as a comment:

- The eight weights sum to `TOTAL_WEIGHT` (1000), so a threshold means the same thing at every call site.
- **No weight exceeds `MAX_SIGNAL_WEIGHT` (250)**, a quarter of the total. So a memory perfect on one signal
  and zero elsewhere cannot reach the top quarter of the range, and the test asserts the comparison directly:
  a memory good on several signals outranks one perfect on a single signal.
- Every listed signal is computable from the record and the query. A row for semantic similarity would be a
  constant zero, which is a weight table claiming a signal it does not have the data to produce.

Two signals the document lists are **deliberately absent**, and their absence is the slice's own title
("before adding embeddings"): **semantic similarity** needs an embedding that does not exist until `P4-005`,
and **active project/task relevance** needs a project or task, which no record carries — a score for either
would be a constant dressed as a signal. A third, **relationship overlap**, is absent because
`entity_relations` exists but nothing traverses it; only direct entity overlap is scored, and the graph walk
is its own slice with its own cost and cycle question.

### 4. The total is the sum of the stored contributions, because integer division is not distributive

The document requires "store component scores and the final inclusion reason". The first implementation
computed the total by scaling the whole weighted sum once, while the per-signal contributions were each
scaled separately — and **the two disagree by up to one per term**, because each division truncates its own
remainder. A test that recomputed the total from the contributions caught it: 521 against 522.

So the total **is** the sum of the contributions. The consequence is that an operator shown the component
scores is reading the arithmetic that produced the total rather than a parallel calculation, which is what
makes the explanation verifiable at all. A displayed breakdown that does not add up is not an explanation.

Integer arithmetic is itself a decision: a float score would make the ranking depend on platform rounding,
so two builds could order the same memories differently and a stored score would be unreproducible.

### 5. One shared scaling helper, because the obvious integer form is always zero

Every signal is a fraction of a bound, and `numerator / denominator` in integers is **zero** for every
partial case. A memory matching two of a question's three words would score as if it matched none, and the
same shape appears in the recency, importance, and reinforcement signals. So one `scaled` helper multiplies
before dividing, and its doc comment states the failure it prevents — eight call sites is eight chances to
forget the multiply or to divide first.

### 6. The inclusion reason is derived from the components, and its precedence is written out

The document requires the reason be **stored**, so the risk is computing it beside the scores and letting
the two disagree — a memory reported as a keyword match while its keyword signal is zero. Deriving it from
the components makes that unrepresentable and makes the reason **verifiable**: the test asserts it against
the signals rather than against a second copy of the rule.

The first implementation picked the strongest matching signal with `max_by_key`, which returns the **last**
maximum on a tie — and a tie is the common case, because a question that names a memory's entity usually
also shares its words, scoring full on all three matching signals. So the reported reason depended on the
order of an array literal. The precedence is now explicit: **identifier, then entity overlap, then keyword**,
with `>` rather than `>=` so the first signal at a given score wins.

A memory that matched nothing is reported as `RecentAndCurrent` or `Important` rather than as a match, and
`SelectionReason::is_a_match` is what exposes the difference. Without it, a caller could present a context
memory as the answer to a question it does not answer.

### 7. Diversification is a prefix of the ranking, and a drop is not a refusal

The per-category caps are what make "diversif[y] across memory types/entities" arithmetic rather than an
intention: a result that filled its allowance with preferences has no room for a preference and therefore
carries something else. Six preferences about one person under a total cap of four is still four preferences
about one person.

The caps are applied **in rank order**, keeping a memory whenever its categories have room. Selecting by
category first — "the best preference, then the best fact" — would override the ranking with a category
order the scoring never expressed, and the document's own rule is that no single signal may dominate: a
category order imposed afterwards is a signal by another name.

A dropped memory and an ineligible one are **separate lists**, because the two are acted on differently. An
ineligible memory cannot be retrieved and reports the rule that refused it; a dropped one was retrievable
and was left out for room. Collapsing them would report "you have seen enough preferences" as "this
preference is not available to you", and an operator acting on the second would go looking for a policy
problem that does not exist.

A **zero** cap is refused rather than interpreted, because "none of this category" and "no limit" are both
plausible readings and they are opposites — and the silent reading would be the restrictive one, making a
category vanish with no explanation.

## Consequences

- **The `CandidateContext` / eligibility split mirrors `P4-003`.** The candidate pipeline decides what may be
  *stored*; eligibility decides what may be *retrieved*. Both are pure functions over borrowed values,
  because entity resolution and the reads need `jarvis-storage` and neither crate may point at the other.
- **A superseded claim is excluded rather than down-ranked**, so `P4-006`'s hybrid scoring inherits a
  candidate set that is already current by construction.
- **The ranking is stable.** Ties break by memory identifier, so two equally-scored memories come back in an
  order that does not depend on the candidate set's arrival order — which for a database-backed read is a
  query plan away from changing between builds.
- **Confidence is a separate eligibility rule from trust**, added beyond the document's six rows. The two
  answer different questions — trust is what the origin can be asked about, confidence is how well this
  claim is supported — and the document's confidence vocabulary exists to be asked. Recorded as an addition
  rather than presented as one of the six.
- **The strict default is a refusal, not a permission.** `MemoryQuery::new` takes `Sensitivity::Internal` as
  the destination ceiling and `Authoritative` as the trust minimum, because a default is what a caller who
  did not think about a field gets, and for a retrieval the permissive default is a disclosure.

## Honest limits

- **Semantic similarity is not implemented.** It is the document's second-ranked signal and there is no
  embedding until `P4-005`. Recorded in the module doc and not in the weights table, so the table does not
  claim a signal it cannot produce.
- **Active project/task relevance is not implemented** — no record carries a project or task.
- **Relationship overlap is not implemented.** Only direct entity overlap is scored; `entity_relations` is
  read by nothing yet, and traversing it is a graph walk with its own cost and cycle question.
- **The recency window is a constant** (thirty days), not a per-query parameter. A window that could vary
  per call would make two scores incomparable and a stored score unreproducible — the same argument that
  makes the arithmetic integer. A caller with a different horizon expresses it as a validity window on the
  claim, which is where a time-bounded fact belongs.
- **The signals are not calibrated against anything.** The relative weights are reasoned rather than
  measured: nobody has yet looked at a real result set and asked whether a keyword match *should* be worth
  200 against an identifier match's 250. `P4-006`'s hybrid scoring and its evaluation are where a
  measurement belongs, and the weights are one table so that measurement is a single edit.
- **Retrieval from the store does not exist.** This is the rules and the arithmetic over records a caller
  already has. No function here issues a query, so nothing yet turns a `MemoryQuery` into a candidate set —
  that is the storage half of the slice, and `P4-002`'s reads are recency-ordered only.
- **Nothing calls this.** No daemon route, no context assembly, no CLI verb. `P4-007` integrates it into
  context budgets, and until then the module is a complete, tested, uninvoked component.

## Alternatives considered

- **Make every eligibility rule a signal with weight zero.** Rejected: a workspace leak and a superseded
  claim are not lower-ranked results, and a weight of zero still admits them through any code path that
  reads the scored list rather than the filtered one.
- **Use floating-point scores.** Rejected: a score would depend on platform rounding, so a stored score
  would be unreproducible and an inclusion reason unverifiable.
- **Normalize the weights against the candidate set.** Rejected: a divisor derived from the candidates would
  make one memory's score depend on the others, which is the coupling the component scores exist to avoid.
- **Compute the reason alongside the scores rather than from them.** Rejected: the two could disagree, and a
  reason that contradicts the scores is worse than no reason because it looks like an explanation.
- **Select by category so the result is guaranteed to contain one of each.** Rejected: it overrides the
  ranking with a category order the scoring never produced, which is a signal that dominates by construction.
- **Treat a zero cap as unlimited.** Rejected: it is the permissive reading of an ambiguous value, and for a
  budget the restrictive reading is the safe one — so the ambiguity must be refused rather than resolved.
- **Exclude a memory that merely touches a saturated entity.** Rejected: the cap bounds how much of the
  result is about one entity, and letting a memory in through a non-saturated second entity would defeat it.
  The conservative direction costs a memory; the permissive one costs the bound.
