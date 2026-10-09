# Integration Research Index

Every external integration requires a dated record based on current official sources. Records describe evidence and decisions; working code and tests remain the proof of implementation.

## Core Sources

| Integration | Official AI/docs index | Record | Status |
| --- | --- | --- | --- |
| MCP | https://modelcontextprotocol.io/llms.txt | [mcp.md](mcp.md) | `P3-007` complete: spec `2026-07-28` and `rmcp` 3.4.0 selected; not implemented |
| Web fetch (`jarvis.web.fetch`) | not applicable; IANA special-purpose registries and the pinned `reqwest` 0.13.5 source | [web-fetch.md](web-fetch.md) | `P3-027` implemented; exfiltration through a model-chosen URL is an open question, held for approval by default |
| ElevenLabs | https://elevenlabs.io/docs/llms.txt | [elevenlabs.md](elevenlabs.md) | architecture researched; both custom-brain transports and turn-taking config recorded; not implemented |
| Twilio | no usable single index; official TwiML/Media Streams/pricing pages | [twilio.md](twilio.md) | architecture researched; not implemented |
| Deepgram (Flux) | https://developers.deepgram.com/llms.txt | [deepgram.md](deepgram.md) | architecture researched; not implemented |
| Google Gemini Live | https://ai.google.dev/llms.txt | [google-gemini-live.md](google-gemini-live.md) | researched; speech-to-speech **not selected** as default |
| LiveKit | https://docs.livekit.io/llms.txt | [livekit.md](livekit.md) | architecture researched; realtime transport, data/RPC, E2EE and Rust SDKs recorded; boundary vs runtime unresolved; session scope undecided; not implemented |
| GitHub Actions / Rust supply chain | https://docs.github.com/llms.txt | [github-actions-rust-supply-chain.md](github-actions-rust-supply-chain.md) | Phase 1 CI researched |
| Rust core primitives | no `llms.txt`; official crates.io/rustdoc | [rust-core-primitives.md](rust-core-primitives.md) | Phase 1 core dependencies researched |
| OS application directories | no `llms.txt`; official OS docs/specs | [os-application-directories.md](os-application-directories.md) | Phase 1 native paths researched |
| Rust configuration | no usable `llms.txt`; official TOML/Serde/crate docs | [rust-configuration.md](rust-configuration.md) | Phase 1 config dependencies researched |
| SQLite / SQLx / Tokio | no usable `llms.txt`; official SQLite, SQLx, Tokio, and crate docs | [sqlite-sqlx.md](sqlite-sqlx.md) | P1-006 implemented and tested on Windows |
| PostgreSQL / pgvector | no usable `llms.txt`; the pgvector README and CHANGELOG plus the SQLx Postgres driver and type docs | [postgres-pgvector.md](postgres-pgvector.md) | `P4-009` partially researched and **partially implemented**: the codec, metric pairing, and the indexed-dimension refusal exist and are tested offline. **SQLx 0.9.0 has no `vector` type mapping**, indexed `vector` is capped at 2,000 dimensions (JARVIS allows 16,384), and `ORDER BY 1 - (v <=> q) DESC` uses no index. **No server was contacted** — Docker is installed but its daemon is not running and no PostgreSQL exists on this machine |
| Rust daemon lifecycle | no usable `llms.txt`; official Rust, Cargo, and Tokio docs | [rust-daemon-lifecycle.md](rust-daemon-lifecycle.md) | P1-007 implemented and tested on Windows |
| Rust structured logging | no usable `llms.txt`; official `tracing` / `tracing-subscriber` rustdoc | [rust-structured-logging.md](rust-structured-logging.md) | implemented with bounded, redacted JSON logs |
| Offline `jarvis doctor` | no usable `llms.txt`; SQLx/SQLite/Rust docs plus repository contracts | [daemon-doctor.md](daemon-doctor.md) | implemented; A02 acceptance case proven |
| Portable mode and service lifecycle | no usable `llms.txt`; systemd/launchd/Windows docs and repository contracts | [portable-and-service-lifecycle.md](portable-and-service-lifecycle.md) | implemented; two concurrent profiles verified isolated |
| Local daemon protocol / IPC | no usable `llms.txt`; official Microsoft named-pipe and `std::os::unix` docs, Tokio `net` docs | [local-daemon-protocol.md](local-daemon-protocol.md) | P1-008 implemented and tested on Windows |
| OAuth 2.0 Authorization Code + PKCE (native apps) | no `llms.txt`; the IETF RFC text files RFC 7636, RFC 8252, RFC 9700, plus RFC 6749 and RFC 9207 as referenced | [oauth2-pkce-native-apps.md](oauth2-pkce-native-apps.md) | `P5-002`: researched from the live RFCs and **implemented** — verifier generation, the single-use authorization transaction, exact redirect matching with the loopback-port exception, rotation detection, and revocation classification. `nonce` is carried but **not validated** (needs OIDC ID-token verification); **no vendor has been called** |
| Google Workspace (identity, Gmail, Calendar, push, quotas, restricted scopes) | **`not found`** — `https://developers.google.com/llms.txt` and `.../gmail/api/llms.txt` both **404**; official guide/reference pages used instead | [google.md](google.md) | `P5-004` researched; `P5-005` **partially implemented** and **no Google API called**. Built: the manifest, the auth flow, the provider's decisions, the tool definitions, the request builders and parsers, the credential boundary, the transport port **and its `reqwest` implementation**, the read operations, the token exchange, revocation, the cursor-signal producer, and nine hand-built fixtures. **Not built: a composition root**, so the transport has no production caller and every fixture proves the *reader* rather than the *record*; the definitions are registered nowhere; no credential exists. Two recorded findings reach outside this integration: **neither Gmail Pub/Sub push nor Calendar channels can be expressed by `WebhookSupport::Push`** (an OIDC bearer JWT and an echoed channel token; the Calendar body is zero-length, so there is nothing to MAC), and **`history.list` signals a stale cursor with an HTTP 404**, the same status as an absent account, so a connector must classify it. Gmail read scopes are all **restricted** (a security assessment if stored or transmitted), the quota model changed **2026-05-01**, and a first-party Gmail MCP server exists in **Developer Preview** |
| OpenAI-compatible model API | https://developers.openai.com/api/docs/llms.txt; https://docs.ollama.com/llms.txt | [openai-compatible-model-api.md](openai-compatible-model-api.md) | P2-001 researched; Chat Completions selected over Responses |
| Embeddings (provider-neutral) | https://developers.openai.com/api/docs/llms.txt; the embeddings guide and create-embeddings reference | [embeddings.md](embeddings.md) | `P4-005` researched; the closed model union, default dimensions (1536/3072), the 8192/300000/2048 bounds, and the prefix-shortening-plus-renormalization rule are recorded; **no provider has been called** |
| Rust HTTP client and SSE | no usable `llms.txt`; `cargo info` plus the official `reqwest` changelog and docs.rs API index | [rust-http-client-and-sse.md](rust-http-client-and-sse.md) | P2-003 dependencies researched; system-proxy default rejected |
| Rust HTTP server and SSE response | no `llms.txt`; official `axum` changelog/README/rustdoc plus `cargo info` and the resolved lock file | [rust-http-server-and-sse.md](rust-http-server-and-sse.md) | P2-007 dependencies researched; `axum` 0.8.9 pinned; HTTP core already resolved via `reqwest` |
| JSON Schema validation | no usable `llms.txt`; crates.io API plus the `jsonschema`/`referencing` source at the version resolved in `Cargo.lock` | [json-schema-validation.md](json-schema-validation.md) | `jsonschema` 0.57.0 selected for `P3-001`; default `resolve-http`/`resolve-file` features disabled so a `$ref` cannot become a network fetch |
| SHA-256 intent hashing | no usable `llms.txt`; crates.io API plus the `sha2` source at the version resolved in `Cargo.lock`, and FIPS 180-4 | [sha2-intent-hashing.md](sha2-intent-hashing.md) | `sha2` 0.10.9 for `P3-004` intent hashes; already in the graph as a `sqlx` dependency |
| `async-trait` object-safe executor | no usable `llms.txt`; crates.io API plus the `async-trait` source at the version resolved in `Cargo.lock` | [rust-async-trait-object-safe-executor.md](rust-async-trait-object-safe-executor.md) | `async-trait` 0.1.92 for `P3-005`; already a workspace dependency via `jarvis-models`, so no new package |
| Filesystem confinement | no usable `llms.txt`; crates.io API plus the `cap-std`/`cap-primitives`/`rustix` source at the versions resolved in `Cargo.lock` | [cap-std-filesystem-confinement.md](cap-std-filesystem-confinement.md) | `cap-std` 4.0.3 for `P3-006`; handle-based confinement because `unsafe_code` is forbidden, so no hand-written `openat`; the canonicalize-then-prefix-check scheme is rejected as a TOCTOU window |
| Console head (face mesh) | no llms.txt; the upstream asset file and Apache-2.0 licence | [mediapipe-canonical-face-model.md](mediapipe-canonical-face-model.md) | MediaPipe canonical face model geometry only, from upstream not example/; 2D canvas rendering, no library |
| OS process sandboxing | no `llms.txt`; official kernel cgroup-v2 documentation and Microsoft Learn job-object documentation | [os-process-sandboxing.md](os-process-sandboxing.md) | `P3-011`: cgroup v2 backend implemented (`pids.max`/`memory.max`/`cpu.max`/`cgroup.kill`), **not executed** on a Linux host here; `CpuTimeCeiling` omitted because cgroup v2 has no cumulative limit file; Windows job objects documented but **not implemented** because every call is `unsafe` and `unsafe_code` is forbidden |
| OpenClaw | https://docs.openclaw.ai/llms.txt | create before adapter implementation | upstream patterns only |
| OpenAI Agents SDK | https://openai.github.io/openai-agents-python/llms.txt | create before adapter implementation | upstream patterns only |
| LangGraph | https://docs.langchain.com/oss/python/langgraph/llms.txt | create before adapter implementation | upstream patterns only |
| Temporal | https://docs.temporal.io/llms.txt | create only when evaluating adapter | deferred |
| Tauri | https://v2.tauri.app/llms.txt | create before desktop scaffold | upstream patterns only |
| Ollama web search (`jarvis.web.search`) | https://docs.ollama.com/llms.txt | [ollama-web-search.md](ollama-web-search.md) | implemented and contract-tested; success path unverified live (needs a key) |

## Planned Connector Records

Create and verify these immediately before their TODO task:

- `microsoft-graph.md`: Microsoft identity, Mail, Calendar, subscriptions, delta queries, throttling
- `github.md`: authentication, REST/GraphQL operations, webhooks, rate limits
- `home-assistant.md`: supported API/auth/event patterns and whether MCP or native API is used
- `ollama.md`: local discovery, chat/embedding APIs, model capabilities, context and lifecycle
- `telnyx.md` or another SIP carrier: only if Twilio is displaced or a second carrier becomes a requirement

## Voice And Telephony Records

The voice path has more than one named provider, and each requires its own dated record before its adapter is written. The records for Twilio, Deepgram (Flux), Google Gemini Live, and LiveKit already exist; see the table above. Turn-detection policy across all of them is recorded in [ADR-0010](../../adr/0010-model-based-turn-detection.md), and the architecture contract is in [voice and telephony](../../architecture/voice-and-telephony.md).

Use [_template.md](_template.md). Never infer current API behavior from the filename or this index.

## Status Vocabulary

- `planned`: no current research; implementation blocked
- `researched`: current contract recorded, no code claim
- `implemented`: adapter and offline contract tests pass
- `live-verified`: opt-in real-provider smoke test passes at the recorded version/date
- `degraded`: provider change or known issue prevents a supported path
- `retired`: unsupported; migration/removal instructions exist