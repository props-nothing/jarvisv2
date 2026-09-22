---
integration: json-schema-validation
status: decided
last_verified: 2026-09-22
owners: []
selected_spec_version: JSON Schema 2020-12
selected_sdk: jsonschema 0.57.0
---

# JSON Schema Validation For Canonical Tool Contracts

## Scope

In scope: choosing a Rust validator for the `input_schema` and `output_schema` of canonical tool
definitions (`P3-001`), and deciding which of its features this project enables. Every tool in
`jarvis-tools` carries both schemas, so this is a dependency reached by all tool code.

Out of scope: JSON Schema *authoring* rules, the MCP tool-list translation (`P3-007`), and any
provider-specific schema dialect (OpenAI's tool schema subset, for example).

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| crates.io API | `https://crates.io/api/v1/crates/jsonschema` | 2026-09-22 | version, license, MSRV, feature list |
| crate manifest | `https://raw.githubusercontent.com/Stranger6667/jsonschema/master/crates/jsonschema/Cargo.toml` | 2026-09-22 | normative feature and dependency graph |
| JSON Schema specification | `https://json-schema.org/draft/2020-12/json-schema-core` | 2026-09-22 | dialect name and vocabulary model |
| repository | `https://github.com/Stranger6667/jsonschema` | 2026-09-22 | `llms.txt`: not found; the manifest is the authoritative feature list |

An `llms.txt` was not found on the project site or repository. The crate manifest was used instead,
which is the same artifact `docs/development/external-research.md` ranks as "official SDK source"
and is more precise than a documentation page for a feature question.

## Verified Contract

### Versions And Deprecations

- Latest is **0.57.0**, published 2026-09-21, **MIT**, `rust-version = 1.85.0`, edition 2021.
- This workspace pins Rust 1.98.1 and edition 2024, so the MSRV is satisfied with room to spare.
- The crate split into `jsonschema-value`, `jsonschema-regex`, `jsonschema-macros`, and
  `referencing` sub-crates at 0.57.x; all are published from the same repository at the same version.
- **0.24.x → 0.57.0 renamed the dialect features.** In 0.24.3 the manifest lists `draft201909` and
  `draft202012` as opt-in features. By 0.57.0 both flags are **gone** and 2019-09/2020-12 are
  supported without a feature gate. Any example that passes `--features draft202012` is describing a
  version this project is not selecting.
- 0.57.0's `default = ["resolve-http", "resolve-file", "tls-aws-lc-rs", "idna"]`.

### Transport And Capabilities (what the features actually gate)

| Feature | Effect | Needed here |
| --- | --- | --- |
| `resolve-http` | pulls `reqwest` + `rustls` to fetch remote `$ref` targets | **no** |
| `resolve-file` | fetches `file:` `$ref` targets from disk | **no** |
| `tls-aws-lc-rs` / `tls-ring` | choose a TLS provider for the above | **no** |
| `idna` | `idn-hostname` and `idn-email` format validation | **no** |
| `arbitrary-precision` | `num-bigint` and exact decimals | **no** |
| `macros` | `#[jsonschema::validator]` compile-time validators | **no** |
| `conformance` | extra conformance checks for custom JSON representations | **no** |

### Local Failure Semantics

Validation is a pure function of (schema, instance) when no external resolution is enabled. There is
no network, no filesystem, and no clock in that configuration, which is what makes it usable inside a
tool-call path that must remain deterministic.

### Verified API Surface (by compiling against 0.57.0, not by reading docs)

Three details were wrong in the first implementation and were corrected by building against the crate:

| Assumption | Reality in 0.57.0 |
| --- | --- |
| `error.kind_keyword()` | `error.kind().keyword()` — `kind()` returns `&ValidationErrorKind`, and `keyword()` is on that |
| `error.instance_path` is a field | It is a `Location`, reached through `error.instance_path()`; it implements `Display` and has `as_str()` |
| `options().with_draft(...).build(...)` is sufficient | It is, but `options().offline()` also exists and sets `OfflineRetriever`, which refuses every fetch. Used in addition to `default-features = false` |

`Location`'s `Display` writes `self.as_str()`, so `instance_path().as_str()` is a JSON pointer
(`/query`), which is the form the canonical violation uses.

## JARVIS Mapping

- `input_schema` / `output_schema` → validated at **registration** time against the 2020-12
  metaschema, and enforced at **call** time against the instance. Two different checks: a schema can
  be valid and still reject good instances, and an instance can be "valid" against a schema that is
  itself malformed.
- Dialect → fixed to `2020-12` by `docs/architecture/tools-and-connectors.md`. A schema declaring
  another `$schema` is refused rather than upgraded, because silently validating against a different
  dialect would accept instances the declared contract rejects.
- Validation failures → a *canonical* error list (pointer + keyword), never the validator's message
  text. The message is a diagnostic, and provider- or library-authored text must not become part of a
  decision or a model-visible payload.

## Decisions

1. **Select `jsonschema` 0.57.0, pinned exactly**, consistent with every other third-party pin in
   this workspace.
2. **`default-features = false` with no features enabled.** The default set enables HTTP and file
   `$ref` resolution. A tool schema is authored by JARVIS or by a connector's manifest, so a `$ref`
   in one must resolve *within the document set JARVIS supplies*, not by fetching a URL. Enabling the
   defaults would make `$ref` a network-fetch primitive inside the tool path, which is the SSRF
   surface `docs/architecture/security.md` lists ("URL parser; scheme/host/IP policy; DNS rebinding
   defense; redirect revalidation") and which nothing in a tool contract requires. Disabling them
   also removes `reqwest`, `rustls`, and `idna` from the `jarvis-tools` graph entirely.
3. **The 2020-12 metaschema is embedded, and the validator is compiled by it.** `P3-001` defines the
   contract; validating schemas against the real metaschema is what makes "JSON Schema 2020-12" a
   check rather than a comment.
4. **`options().offline()` is set as well as `default-features = false`.** Two mechanisms, because
   they fail differently: the feature set is a property of `Cargo.toml` that a later edit can change
   without touching the tool path, while `.offline()` states the refusal at the call site. A test
   asserts the *specific* `ExternalReference` error rather than merely "building failed", so the
   reason survives a change to the feature set.
5. **`format` is annotated, not asserted** (`.should_validate_formats(false)`). 2020-12 makes format
   annotation-only, and asserting it would cover only the formats the library implements — so a tool
   declaring `"format": "email"` would silently accept what its author expected to be refused. A tool
   constrains shape with `pattern`, which is always checked, and treats reachability of an address as
   something only the provider can establish. A test fixes this so it cannot change quietly.

## Rejected Alternatives

- **Validate schemas by hand (a keyword allowlist).** Rejected: a hand-written subset would silently
  accept or reject keywords differently from the specification, and "which subset do we implement" is
  a permanent maintenance answer with no upper bound. A schema language is exactly the case for using
  the specification's own implementation.
- **Enable `resolve-http` for connector-supplied schemas.** Rejected for the reason above: it turns
  schema validation into an outbound HTTP client inside the policy path. A connector manifest that
  needs a shared schema must inline it or reference a sibling document JARVIS controls.
- **`macros` for compile-time validators.** Rejected for now: it adds a proc-macro dependency and a
  build-time schema parse to save a registration-time parse that happens once per tool.
- **Defer the dependency and ship `P3-001` without schema validation.** Rejected: `P3-002`'s registry
  must reject a malformed schema at registration, and a registry that accepted any JSON as a schema
  would make the whole capability layer's contract unenforceable.

## Unresolved

- **`unevaluatedProperties` support.** The 2020-12 vocabulary requires annotation-dependent
  evaluation; the crate advertises support but this record has not verified it against a live case.
  A test asserts the behaviour this project relies on (`additionalProperties` and `required`), and a
  tool schema needing `unevaluated*` is not yet written.

## Resolved In P3-002

Both items this record previously left open are now decided and verified by a contract test
(`crates/jarvis-tools/tests/supplied_documents.rs`):

- **Resolving a shared definition without fetching.** Verified by running, not by reading: with the
  resolver's default features disabled **and** `.offline()` set, a `$ref` to a document supplied
  through `with_registry` resolves **and its constraints are enforced** (`minLength` and `maxLength`
  from the supplied document both apply). The control case — the same reference with nothing supplied
  — is refused. Both halves are asserted, because the permissive case alone would also pass on a build
  that fetched the URL.
- **A sibling-file `$ref`.** Still refused, now deliberately rather than by omission: the walk does not
  distinguish a local path from a URL, and a path is a filesystem primitive inside the tool path. A
  schema may compose only against a `DocumentSet` keyed by `$id`, never by path.

Verified API details for this (0.57.0 / `referencing` 0.57.0, both read from the vendored source):
`Registry::new()` returns a `RegistryBuilder`; add resources with
`.add(uri, Resource::from_contents(value))` and finish with `.prepare()`. `Resource::from_contents`
returns a `Resource` directly (no `Result`), and `options().with_registry(&registry)` on the
`jsonschema` side copies what it needs — `Validator` carries no lifetime, so the registry does not
have to outlive the validator.

## License Note

`jsonschema` is MIT, but its graph brings `MIT-0` (`borrow-or-share` via `referencing` ->
`fluent-uri`). `MIT-0` is MIT without the attribution clause, so it is strictly less restrictive than
`MIT`, already allowed. Added to `deny.toml`'s allow list with that reasoning recorded.
