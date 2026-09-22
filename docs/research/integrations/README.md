# Integration Research Index

Every external integration requires a dated record based on current official sources. Records describe evidence and decisions; working code and tests remain the proof of implementation.

## Core Sources

| Integration | Official AI/docs index | Record | Status |
| --- | --- | --- | --- |
| MCP | https://modelcontextprotocol.io/llms.txt | [mcp.md](mcp.md) | `P3-007` complete: spec `2026-07-28` and `rmcp` 3.4.0 selected; not implemented |
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
| Rust daemon lifecycle | no usable `llms.txt`; official Rust, Cargo, and Tokio docs | [rust-daemon-lifecycle.md](rust-daemon-lifecycle.md) | P1-007 implemented and tested on Windows |
| Rust structured logging | no usable `llms.txt`; official `tracing` / `tracing-subscriber` rustdoc | [rust-structured-logging.md](rust-structured-logging.md) | implemented with bounded, redacted JSON logs |
| Offline `jarvis doctor` | no usable `llms.txt`; SQLx/SQLite/Rust docs plus repository contracts | [daemon-doctor.md](daemon-doctor.md) | implemented; A02 acceptance case proven |
| Portable mode and service lifecycle | no usable `llms.txt`; systemd/launchd/Windows docs and repository contracts | [portable-and-service-lifecycle.md](portable-and-service-lifecycle.md) | implemented; two concurrent profiles verified isolated |
| Local daemon protocol / IPC | no usable `llms.txt`; official Microsoft named-pipe and `std::os::unix` docs, Tokio `net` docs | [local-daemon-protocol.md](local-daemon-protocol.md) | P1-008 implemented and tested on Windows |
| OpenAI-compatible model API | https://developers.openai.com/api/docs/llms.txt; https://docs.ollama.com/llms.txt | [openai-compatible-model-api.md](openai-compatible-model-api.md) | P2-001 researched; Chat Completions selected over Responses |
| Rust HTTP client and SSE | no usable `llms.txt`; `cargo info` plus the official `reqwest` changelog and docs.rs API index | [rust-http-client-and-sse.md](rust-http-client-and-sse.md) | P2-003 dependencies researched; system-proxy default rejected |
| Rust HTTP server and SSE response | no `llms.txt`; official `axum` changelog/README/rustdoc plus `cargo info` and the resolved lock file | [rust-http-server-and-sse.md](rust-http-server-and-sse.md) | P2-007 dependencies researched; `axum` 0.8.9 pinned; HTTP core already resolved via `reqwest` |
| JSON Schema validation | no usable `llms.txt`; crates.io API plus the `jsonschema`/`referencing` source at the version resolved in `Cargo.lock` | [json-schema-validation.md](json-schema-validation.md) | `jsonschema` 0.57.0 selected for `P3-001`; default `resolve-http`/`resolve-file` features disabled so a `$ref` cannot become a network fetch |
| SHA-256 intent hashing | no usable `llms.txt`; crates.io API plus the `sha2` source at the version resolved in `Cargo.lock`, and FIPS 180-4 | [sha2-intent-hashing.md](sha2-intent-hashing.md) | `sha2` 0.10.9 for `P3-004` intent hashes and nonce digests; already in the graph as a `sqlx` dependency |
| `async-trait` object-safe executor | no usable `llms.txt`; crates.io API plus the `async-trait` source at the version resolved in `Cargo.lock` | [rust-async-trait-object-safe-executor.md](rust-async-trait-object-safe-executor.md) | `async-trait` 0.1.92 for `P3-005`; already a workspace dependency via `jarvis-models`, so no new package |
| Filesystem confinement | no usable `llms.txt`; crates.io API plus the `cap-std`/`cap-primitives`/`rustix` source at the versions resolved in `Cargo.lock` | [cap-std-filesystem-confinement.md](cap-std-filesystem-confinement.md) | `cap-std` 4.0.3 for `P3-006`; handle-based confinement because `unsafe_code` is forbidden, so no hand-written `openat`; the canonicalize-then-prefix-check scheme is rejected as a TOCTOU window |
| OpenClaw | https://docs.openclaw.ai/llms.txt | create before adapter implementation | upstream patterns only |
| OpenAI Agents SDK | https://openai.github.io/openai-agents-python/llms.txt | create before adapter implementation | upstream patterns only |
| LangGraph | https://docs.langchain.com/oss/python/langgraph/llms.txt | create before adapter implementation | upstream patterns only |
| Temporal | https://docs.temporal.io/llms.txt | create only when evaluating adapter | deferred |
| Tauri | https://v2.tauri.app/llms.txt | create before desktop scaffold | upstream patterns only |

## Planned Connector Records

Create and verify these immediately before their TODO task:

- `google.md`: Google Identity, Gmail, Calendar, push notifications, quotas, restricted scopes
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