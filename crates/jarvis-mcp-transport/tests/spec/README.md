# Vendored spec-schema slice

`2026-07-28-result-slice.json` is a **derived slice** of the official Model Context Protocol JSON Schema for
the `2026-07-28` revision. It exists so the conformance test in `src/conformance.rs` can validate this
server's **emitted** results against the protocol's own schema instead of against `rmcp` — the SDK this server
is built from, and therefore the thing under test.

## Provenance

| | |
| --- | --- |
| Source | `https://raw.githubusercontent.com/modelcontextprotocol/modelcontextprotocol/main/schema/2026-07-28/schema.json` |
| Fetched | 2026-09-23 (181,474 bytes) |
| Licence | Apache-2.0, as the specification it is part of |
| Modification | **None to any definition.** The file is a subset of `$defs`, byte-identical per definition. |
| Extraction | [`extract.ps1`](extract.ps1) — the transitive `#/$defs/<Name>` closure from the roots below |

## Why a slice rather than the whole document

The full schema is ~181 KB and defines every type in the protocol. This server emits four result types, and
the definitions unreachable from those cannot change a verdict. Vendoring the whole document would put a
large generated file in the tree to answer a question 11 definitions answer.

The slice is computed by **reference closure**, not by hand. A hand-picked set is how a slice quietly stops
covering a field that changes: nobody notices a definition that *should* have been added. `extract.ps1`
recomputes the closure, and `the_slice_is_closed_under_reference` in `src/conformance.rs` fails if the file
ever references a definition it does not carry.

## Regenerating

```powershell
# from the repository root
Invoke-WebRequest -Uri "https://raw.githubusercontent.com/modelcontextprotocol/modelcontextprotocol/main/schema/2026-07-28/schema.json" -OutFile spec-full.json -UseBasicParsing
powershell -NoProfile -ExecutionPolicy Bypass -File crates\jarvis-mcp-transport\tests\spec\extract.ps1 -Source spec-full.json -Destination crates\jarvis-mcp-transport\tests\spec\2026-07-28-result-slice.json
Remove-Item spec-full.json
```

Then update the "Fetched" date above and the byte count if it changed.

**Regenerate when the revision's schema changes, and treat a change as a finding**: this slice is the
authority a conformance claim rests on, so a silent update is how a test stops testing what it says.

## What is in it

Roots: `ListToolsResult`, `DiscoverResult`. The closure pulls in `Tool`, `ToolAnnotations`, `Icon`,
`Implementation`, `ServerCapabilities`, `MetaObject`, `ResultMetaObject`, `JSONObject`, and `JSONValue`.

The interesting field is `ttlMs`/`cacheScope`: `ListToolsResult` and `DiscoverResult` both declare
`"required": ["cacheScope", "resultType", "ttlMs", …]` in this revision, which is the requirement `rmcp`
deliberately does not satisfy by default — see `src/serve.rs`'s module documentation. That divergence is why
the authority here has to be the specification rather than the SDK.
