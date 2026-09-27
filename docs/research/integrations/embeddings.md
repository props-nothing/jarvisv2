---
integration: embeddings
status: researched
last_verified: 2026-09-27
owners: []
selected_spec_version: OpenAI Embeddings API v1 (`POST /v1/embeddings`)
selected_sdk: none (hand-written HTTP against the OpenAI-compatible wire format)
---

# Embeddings (provider-neutral)

## Scope

The operations researched: **creating embedding vectors for text, and the metadata required to store and
compare them safely.** That is the whole of `P4-005`: a provider-neutral port over "turn text into a
vector", plus the dimension/version metadata `docs/architecture/storage.md` requires before two vectors may
be compared.

Out of scope, deliberately:

- **Vector search itself.** SQLite has no vector index here, `docs/data/schema.md` allows "a selected local
  extension or an application-managed index", and `P4-006` owns hybrid retrieval and scoring. This slice
  produces vectors and their provenance.
- **Chunking.** `docs/data/schema.md` names a `chunker version` and a `document_embeddings` table for
  documents. Document ingestion is not a Phase 4 slice; the field is recorded as an unresolved question
  below rather than invented.
- **Image, audio, and multimodal embeddings.** The researched provider API is text-only, and no slice needs
  them.
- **Re-indexing as a durable job.** `docs/architecture/storage.md` requires re-indexing be "durable [and]
  resumable"; that is a job-runner concern and is recorded as a question this slice does not answer.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| OpenAI API docs index | https://developers.openai.com/api/docs/llms.txt | 2026-09-27 | discovery; the embeddings guide and reference are both listed |
| Embeddings guide (normative for models, dimensions, normalization) | https://developers.openai.com/api/docs/guides/embeddings.md | 2026-09-27 | model table, default dimensions, shortening, distance function |
| Create-embeddings reference (normative for the request) | https://developers.openai.com/api/reference/resources/embeddings/methods/create | 2026-09-27 | request parameters, response shape, limits |
| Chat Completions record (same provider, adjacent slice) | `docs/research/integrations/openai-compatible-model-api.md` (2026-09-21) | 2026-09-27 | transport, auth header, error classes already established |

`docs/research/integrations/README.md` lists `ollama.md` as a **planned** record for "local discovery,
chat/embedding APIs, model capabilities, context and lifecycle". It does **not** exist, and this slice did
not create it: no slice has yet named a local embedding provider, and writing a record for a provider
nothing selects would be research for its own sake. The provider-neutral port below is what makes that
record a later, additive change.

The OpenAI API's Markdown twins are at `/api/docs/<slug>.md` and `/api/reference/...`, which is what the
index itself advertises. Both pages were fetched live.

## Verified Contract

### Operations And Transport

- **`POST /v1/embeddings`**, JSON body, `Authorization: Bearer <key>`. Same transport and auth as the Chat
  Completions record, which is why this record does not restate them.
- **Request parameters** (all verified from the reference page):
  - `input` — `string`, `array of string`, `array of number`, or `array of array of number`. An **array of
    token arrays** is accepted, which means the API accepts inputs this port will never send: a token array
    is provider-tokenizer-specific and cannot be constructed from text by a provider-neutral caller.
  - `model` — the model id. The reference enumerates an `EmbeddingModel` union of exactly
    `text-embedding-ada-002`, `text-embedding-3-small`, `text-embedding-3-large`.
  - `dimensions` — **optional number**, "Only supported in `text-embedding-3` and later models."
  - `encoding_format` — `float` (the practical default) or `base64`.
  - `user` — optional safety identifier for abuse monitoring.
- **Response**: `CreateEmbeddingResponse { data, model, object, usage }` where
  - `data` is an array of `Embedding { embedding: array of number, index: number, object: "embedding" }`,
  - `model` is the model actually used, echoed back,
  - `object` is `"list"`,
  - `usage` is `{ prompt_tokens, total_tokens }`.
- **`index` is the join back to the input.** The reference defines it as "the index of the embedding in the
  list of embeddings", so a batched request must be reassembled by `index` rather than by position — an
  ordering assumption that holds today would silently mis-assign vectors if it ever changed.
- **One request, no streaming.** There is no SSE or partial-result mode.

### Authentication And Authorization

- `Authorization: Bearer $OPENAI_API_KEY`, identical to the Chat Completions record. No scopes: the API key
  carries the grant, so least privilege is a separate key, not a narrower scope. The key value never leaves
  the adapter, per `AGENTS.md`.

### Limits And Failure Semantics

Verified from the reference, quoted:

- **8192 tokens** maximum input "for all embedding models".
- **An empty string is refused** — "cannot be an empty string". This is a client-side refusal the adapter
  can make before spending a request.
- **300,000 tokens summed across all inputs** in a single request, "In addition to the per-input token
  limit".
- **An input array is bounded at 2048 entries** — "any array must be 2048 dimensions or less" (the page uses
  "dimensions" here to mean array elements).
- **Pricing is per input token**; the guide's model table gives the max input as 8192 for all three models.
- **No idempotency key** is documented for this endpoint. That matters: a retried request after an ambiguous
  outcome produces a second billed call. An embedding is **read-only and idempotent in effect** (the same
  text and model produce the same vector), so a retry is safe in *effect* but not free in *cost* — which is
  the distinction to record, and it is why retry policy belongs to the caller rather than the adapter.

### Data And Compliance

- The guide's FAQ states customers "own their input and output from our models, including in the case of
  embeddings". The source text is therefore not encumbered by the provider, but it **does leave JARVIS**,
  so `Sensitivity` and destination policy apply exactly as they do to a chat completion: `P4-004`'s
  eligibility already refuses a memory above a destination's ceiling, and the same ceiling must gate
  embedding calls once a memory is embedded.
- **No retention guarantee is stated for the embeddings endpoint in the pages read.** The guide does not
  make a zero-retention claim. Recorded as an unresolved question below rather than assumed, because
  `docs/architecture/memory-and-context.md` requires "provider-side deletion requirements are surfaced
  separately" and that is not answerable from what was read.
- **Pricing is token-based** and the guide publishes approximate pages-per-dollar figures. There is no free
  tier claim on the pages read.

### Versions And Deprecations

- **Three models exist** and the reference enumerates them as a closed union: `text-embedding-ada-002`,
  `text-embedding-3-small`, `text-embedding-3-large`.
- **Default dimensions are 1536 for `text-embedding-3-small` and 3072 for `text-embedding-3-large`**, from
  the guide. The guide's example response for `text-embedding-ada-002` notes "1536 floats total for
  ada-002", so all three default to 1536 at minimum and the two v3 models differ.
- **`dimensions` shortens a vector and the result is a prefix.** The guide: developers "can shorten
  embeddings (i.e. remove some numbers from the end of the sequence) without the embedding losing its
  concept-representing properties by passing in the `dimensions` API parameter", and recommends the
  parameter over manual truncation. The guide's manual example is literally `slice(0, 256)`.
- **Shortening is normalized by the API, truncation by hand is not.** The guide's manual path computes an
  L2 magnitude and divides, with the zero-magnitude case guarded. That is the single most important fact in
  this record for JARVIS: **a shortened vector and a truncated one are not the same value**, so an adapter
  that hand-truncates to a target dimension produces vectors that do not compare correctly with ones the
  API shortened.
- **A `text-embedding-3-large` vector shortened to 256** still outperforms an unshortened
  `text-embedding-ada-002` (1536) on MTEB, per the guide — so dimension reduction is a cost lever with a
  measured quality floor rather than a guessing game.
- **Stability is not claimed.** Nothing read states that a given model id returns a stable vector across
  versions or over time, and the guide's FAQ says the v3 models "lack knowledge of events that occurred
  after September 2021" without addressing model-version drift. This is why the port carries a version
  rather than only a model name.
- **Distance function**: the guide recommends **cosine similarity**, and states OpenAI embeddings "are
  normalized to length 1, which means that cosine similarity can be computed slightly faster using just a
  dot product [and] cosine similarity and Euclidean distance will result in the identical rankings".
  **That normalization claim is provider-specific and is not part of the wire format.**

## JARVIS Mapping

| Provider concept | JARVIS type | Notes |
| --- | --- | --- |
| model id | `EmbeddingModel` (validated like `ModelId`) | the port's selectable capability |
| `dimensions` | `EmbeddingDimensions` (a bounded newtype) | carried in the metadata, not just the request |
| `encoding_format` | not mapped | the port never sends `base64`; see Decisions |
| `index` | reassembly key | vectors are matched to inputs by `index`, and a gap is a provider fault |
| `embedding` | `EmbeddingVector` | bounded `Vec<f32>`, length must equal the declared dimensions |
| `usage.total_tokens` | `TokenUsage` | reuses `jarvis-models`' existing type so cost accounting is one shape. The adapter reads `prompt_tokens`, which the guide documents as both fields for this endpoint |
| model/version/dimensions/normalization/input hash | `EmbeddingMetadata` | the comparison guard `storage.md` requires |

`EmbeddingMetadata` is the load-bearing mapping. `docs/architecture/storage.md` says: "Store embedding
provider/model, dimensions, normalization, input hash, chunker version, and created time. **Never compare
vectors with incompatible metadata.**" So the metadata is not decoration — it is the precondition for a
comparison, and the type makes the comparison itself a checked operation.

## Decisions

1. **A separate port, not a method on `ModelGateway`.** Chat and embeddings are different operations with
   different request shapes, different limits, and different failure meanings; a combined trait would make
   every chat adapter implement an embedding method it may not support, and an unimplemented method is a
   runtime refusal where a separate port is a compile-time one.
2. **`encoding_format` is not exposed.** The port sends `float`. The reference's only other value is
   `base64`, and `float` is what the guide's own curl examples use. A `base64` path would be an optimization
   with no measured need, and an unmeasured code path that decodes provider-supplied base64 is one to avoid.
3. **`dimensions` is a request field *and* stored metadata.** Sending it and not storing it would let a
   256-dimension vector compare against a 1536-dimension one. The port carries the requested dimension
   through to `EmbeddingMetadata`.
4. **Normalization is stored as a declared fact, and JARVIS's own default is "not normalized".** The guide
   states OpenAI vectors are length 1, which makes cosine and dot product interchangeable *for that
   provider*. A provider-neutral port cannot assume it, so `Normalization` is a field with
   `Unknown` as its **default** — a default is what a caller who did not think about it gets, and assuming
   normalization is the permissive assumption when comparing distances.
5. **Inputs are refused for emptiness before a request is spent.** The API refuses an empty string; refusing
   it locally turns a billed round trip into a local error, and the error names the input index.
6. **The port has one batch method, and it exists because the limits differ.** The wire accepts an array and
   bounds it at 2048 entries and 300,000 tokens total; neither bound is knowable per-input without a
   tokenizer this crate does not have. So the method takes the batch the caller assembled and the adapter
   reports the provider's refusal rather than pre-computing a count it cannot compute.
7. **No retry inside the adapter.** The Chat Completions record already establishes that "`complete`
   performs exactly one provider attempt" and that retry policy is the caller's. Same rule here, with the
   extra note that a retry costs tokens.

## Rejected Alternatives

- **A `ModelGateway::embed` method.** Rejected: it forces every chat adapter to implement an operation it
  may not support, and turns a compile-time absence into a runtime refusal.
- **Reusing `ChatRequest` with an embedding flag.** Rejected: the request shapes share nothing but a model
  id, and a flag makes an invalid combination representable (an embedding request with messages).
- **Storing vectors without metadata and trusting the caller to compare like with like.** Rejected
  directly by `docs/architecture/storage.md`'s "Never compare vectors with incompatible metadata", and this
  is the defect class `P4-002`'s `ADR-0044` already describes: two values that must agree with nothing
  holding both. `EmbeddingMetadata` holds both.
- **Assuming normalization because the researched provider normalizes.** Rejected: the port is
  provider-neutral, and a provider that returns unnormalized vectors would then be compared by dot product
  and rank wrongly. The field defaults to `Unknown`, so the permissive assumption is not the default.
- **Deriving the dimension from the returned vector's length and discarding the requested value.**
  Rejected: it makes dimension drift undetectable. Storing the request *and* checking the response against
  it turns "the provider returned a different shape than asked for" into a named failure.
- **A token-count estimate to pre-check the 300,000-token batch bound.** Rejected: this crate has no
  tokenizer, the guide points at `tiktoken` for `cl100k_base`, and an estimate used as a *limit* would
  refuse valid requests or admit invalid ones while looking like a check.
- **Hand-truncating vectors to a target dimension.** Rejected on the guide's own evidence: manual
  shortening requires L2 renormalization, and the `dimensions` parameter is "the suggested approach". A
  hand-truncated vector is a different value from an API-shortened one.
- **A `base64` encoding path.** Rejected as an unmeasured optimization that adds a decode path for
  provider-supplied data.
- **Making the vector a `Vec<f64>`.** Rejected: the wire carries JSON numbers and every practical vector
  store uses `f32`; `f64` doubles the storage for precision the provider does not provide.

## Verification Plan

- **Offline contract tests against a scripted transport**, mirroring the Chat Completions record's approach:
  the adapter is driven through the existing `Transport` seam so no network is touched, and the tests assert
  the request body's exact shape (model, input, dimensions present only when set, `encoding_format: float`)
  and the parse of a sanitized fixture response.
- **A fixture must be sanitized**: the guide's own example response is used as the shape (it contains no
  real data), and the vector values are synthetic.
- **Cheapest test that would disprove the central assumption**: assert that the adapter **refuses** to
  compare, or to assemble metadata for, a vector whose length differs from the declared dimensions. If that
  passes while a mismatched vector is still comparable, the whole metadata guard is decorative. The
  central assumption is "the metadata prevents an incompatible comparison", and the falsification is a
  vector whose length contradicts its metadata being rejected rather than silently accepted.
- **Opt-in live smoke test** (not run in this slice): one text embedded by one model, asserting the
  returned length equals the declared default dimension for that model. This is what would move the record
  to `live-verified`, and it cannot be done offline because the dimension is a provider fact.

## Unresolved Questions

1. **Is there a documented zero-retention or no-training guarantee for the embeddings endpoint?** The pages
   read make no such claim. Impact: a privacy statement about embedded memory cannot be written from it.
   Blocks: any user-facing claim about where embeddings go.
2. **Does a model id return a stable vector across time, or does a provider-side update change it?**
   Nothing read claims stability. Impact: a stored vector could silently become incomparable with a fresh
   one. Blocks: nothing in this slice (the metadata carries a version precisely so a mismatch is
   detectable), but `P4-006`'s scoring must decide what to do when versions differ.
3. **What is the correct token estimate for the 8192 and 300,000 bounds without a tokenizer?** The guide
   names `tiktoken` / `cl100k_base` and provides no count endpoint for this API. Impact: the adapter cannot
   pre-check a batch, so an over-limit batch fails at the provider. Blocks: a client-side batch validator,
   which is deliberately not built.
4. **What is the "chunker version" for, concretely?** `docs/architecture/storage.md` requires storing one
   and `docs/data/schema.md` puts it on `document_embeddings`, but no document-ingestion slice exists.
   Impact: the field is stored as an opaque value on the memory path where it is currently constant.
   Blocks: document embedding, which is not a Phase 4 slice.
5. **Which local provider, if any, will serve embeddings offline?** No slice names one, and `ollama.md` is
   planned rather than written. Impact: the port's second implementation is unplanned. Blocks: an offline
   end-to-end embedding test; the scripted adapter is what stands in for it here.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-27 | Embeddings API v1; models `text-embedding-ada-002`, `text-embedding-3-small`, `text-embedding-3-large` | First record. Fetched the API docs index, the embeddings guide, and the create-embeddings reference live. Confirmed the closed model union, default dimensions (1536 / 3072), the 8192-token and 300,000-token and 2048-element bounds, the empty-string refusal, the `index` field, the `usage` shape, and the prefix-shortening-plus-L2-normalization rule. | Copilot |
