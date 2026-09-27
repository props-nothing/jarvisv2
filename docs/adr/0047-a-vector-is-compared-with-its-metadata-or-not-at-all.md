# ADR-0047: A vector is compared with its metadata or not at all, and the vector is not a number

**Status:** Accepted

**Date:** 2026-09-27

## Context

`P4-005` adds a provider-neutral embedding port, following the research record in
`docs/research/integrations/embeddings.md`. The external contract is small — one operation, one request
shape, an optional dimension — so the interesting decisions are not about the wire. They are about what
`docs/architecture/storage.md` demands of the *result*:

> Store embedding provider/model, dimensions, normalization, input hash, chunker version, and created time.
> **Never compare vectors with incompatible metadata.**

That second sentence is a rule about a **pair** of values. The document also names the fields, which makes
the rule look like a checklist a caller can follow — and a checklist is exactly the shape that fails, because
the two fields easiest to forget are the two that produce a plausible wrong answer:

- A **normalization** mismatch does not fail, it ranks wrongly. A dot product over an unnormalized vector and
  a normalized one returns a number in the right range often enough to look like a score.
- A **dimension** mismatch does fail, but it fails as a length error far from the comparison, which reads as
  a corrupt vector rather than as two incomparable ones.

There was also a temptation to treat the vector as the operation: a free function
`cosine_similarity(a: &[f32], b: &[f32]) -> f32`. That signature is the defect, because it cannot see either
field.

## Decision

### 1. The comparable value is the pair, so the pair is a type

`Embedding` holds an `EmbeddingVector` and an `EmbeddingMetadata` together, and the only way to obtain one is
`Embedding::new`, which validates the metadata and refuses a vector whose length disagrees with the declared
dimension. **A vector whose length contradicts its own metadata cannot be constructed at all**, so it cannot
reach storage, a comparison, or a serialized form.

### 2. `ensure_comparable_with` is the guard, and it is a method on the pair

The rule from the architecture document is implemented as a method taking the other metadata, not as advice
in a doc comment and not as six `if` statements for a caller to remember. It checks in order of how
misleading the mismatch is — provider, model, version, dimensions, normalization — and returns
`IncompatibleMetadata { field }` naming the **first** field that differed, so a failure is actionable rather
than a boolean. The order is not arbitrary: a provider or model mismatch means the two vectors were produced
by different functions entirely, whereas a normalization mismatch is the one that still returns a
plausible-looking number.

### 3. Normalization is a declared fact, and "unknown" is the default

`Normalization` has three states — `Normalized`, `Unnormalized`, `Unknown` — and `Unknown` is
`#[default]`. The cosine helper takes the dot-product shortcut **only** when both sides declare
`Normalized`, and computes magnitudes otherwise. A caller who never thought about the field gets the safe
path; a caller who declares it wrongly gets the shortcut, but that is a positive claim rather than an
omission.

The researched provider does normalize. The port is still provider-neutral, because the port is what a
second provider has to satisfy, and "the one provider I read about normalizes" is not a property of a port.

### 4. The port is separate from `ModelGateway`

`EmbeddingGateway` is its own trait with its own request, response, and error meanings. A combined gateway
would force every chat adapter to implement an embedding operation it may not support, and an unimplemented
trait method is a runtime refusal where a separate port is a compile-time one.

### 5. Vectors are reassembled by the provider's `index`, never by position

The response is a list, and the request is a list. Zipping them positionally is correct only if the provider
returns them in order and never omits one — neither of which is promised. The adapter places each vector at
the index the provider reported, refuses a duplicate or out-of-range index, and refuses a response that does
not cover every input. A provider that returned the vectors shuffled would otherwise pair every text with
somebody else's vector, silently, and the resulting scores would be meaningless rather than wrong.

### 6. `input_hash` and `chunker_version` are stored but deliberately not compared

`ensure_comparable_with` stops before these two. The document's list is a set of properties a *stored* vector
must carry; it is not a set of fields that must be equal for a comparison. Two vectors of **different** text
are precisely what a similarity search compares, so requiring the input hash to match would make the only
meaningful comparison impossible. They are recorded so a re-index can find the vectors a change invalidated —
which is a different question from whether two vectors may be compared.

### 7. Error messages name an index, never the input text

A batch of two thousand texts makes "one of these is empty" unactionable, so the refusal names the index of
the offending input. It names the index and not the text, because the text is user content and the message
would reach a log.

## Consequences

- **The defect class `ADR-0044` described is closed at the type level.** `P4-002`'s finding was "two values
  that must agree, with nothing holding both". Here something holds both, and the constructor is the only
  way to make one.
- **A dimension change is a named failure rather than drift.** Because the requested dimension is stored as
  metadata rather than inferred from the returned vector's length, a provider that silently returns a
  different shape is refused — where deriving the dimension from the response would have made the drift
  undetectable and permanent.
- **The port produces no similarity score.** It produces vectors and metadata. Ranking is `P4-006`'s, and
  keeping the score out of the adapter is what lets the scoring be pure and testable without a transport.
- **The cost of the guard is one comparison per similarity call.** It is a field-by-field comparison of small
  values, negligible against the vector arithmetic it precedes, and it makes the guard impossible to skip by
  calling a free function.
- **A malformed response is classified without its text.** The adapter reuses the Chat Completions record's
  error classification, so a body that cannot be interpreted becomes `MalformedResponse` with a static
  message rather than an echoed fragment of a provider body.

## Honest limits

- **No provider has been called.** The record stays `researched`, not `live-verified`. The provider's
  normalization claim, the default dimension per model, and the empty-string refusal are read from the
  documentation; a live smoke test is described in the research record's verification plan and is not run
  here. The offline tests drive a scripted transport, so they prove the request shape and the parse, not the
  provider's behavior.
- **Nothing writes an embedding to storage.** There is no `memory_embeddings` table and no repository
  function. The vector search half — pgvector parity and the storage path — is `P4-009`, and the storage half
  of this slice is deliberately absent.
- **No chunking.** The port embeds whatever text it is handed, and a long text is the caller's problem. The
  8192-token limit is not pre-checked, because checking it needs a tokenizer this crate does not have.
- **No token-count pre-check for the batch bound either.** The adapter reports the provider's refusal rather
  than computing a count it cannot compute; an estimate used as a limit would refuse valid requests while
  looking like a check.
- **No retry inside the adapter.** One attempt, matching the Chat Completions record, with the extra note
  that a retry costs tokens.
- **`chunker_version` is `None` on the memory path**, where it is constant because nothing chunks yet.
- **`base64` is not supported.** The port sends `float` only, and an unmeasured decode path for
  provider-supplied data was not worth adding.
- **Nothing calls this.** No daemon route, no memory path, no CLI verb — the same status `P4-004`'s retrieval
  module has, and `P4-006` is what starts consuming it.

## Alternatives considered

- **A free `cosine_similarity(a, b)` function.** Rejected: it cannot see the metadata, so the architecture
  document's rule would become advice. The signature is the bug.
- **Validation at the storage boundary instead of at construction.** Rejected: it makes two guards (one at
  write, one at compare) where one suffices, and the one that catches the defect earliest is the constructor.
- **A caller-side `metadata_matches(a, b)` helper.** Rejected: every caller would have to remember all six
  fields, and the two that matter most are the two a caller is least likely to think of.
- **Trusting the researched provider's normalization.** Rejected: the port is provider-neutral, and
  `Normalization::Unnormalized` defaults rather than `Normalized` for the same reason — the permissive
  assumption on a distance ranks wrongly instead of failing.
- **Returning an `Embedding` from the gateway without a length check.** Rejected: it makes the metadata
  describe something the vector is not, which is worse than no metadata because it looks checked.
- **Inferring the dimension from the returned vector's length.** Rejected: it makes a provider-side shape
  change undetectable and unrecoverable, and the requested dimension is information the caller already had.
- **Comparing the input hash in `ensure_comparable_with`.** Rejected: it forbids every comparison a search
  actually needs to make.
- **A `Vec<f64>` vector.** Rejected: the wire carries JSON numbers and every practical vector store uses
  `f32`; `f64` doubles the storage for precision the provider does not provide.
- **Storing a `Normalization` for the whole provider rather than per vector.** Rejected: a provider can change
  it, and a per-vector value is what makes a stored vector self-describing.
