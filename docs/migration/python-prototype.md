# Python Prototype Migration

## Status And Legal Boundary

The [example](../../example/readme.md) is a substantial MARK LIV Python assistant and behavior reference. Its README identifies a CC BY-NC 4.0 license. The future JARVIS platform has no selected license yet.

Therefore:

- keep `example/` quarantined
- do not mechanically port or copy code into the Rust platform
- specify behavior and invariants in neutral language
- implement clean-room replacements from those requirements and official upstream docs
- retain attribution and third-party asset notices inside the example
- obtain explicit legal review/permission before any source reuse

## Current Shape

| Prototype area | Files | Current responsibility |
| --- | --- | --- |
| Main process | [main.py](../../example/main.py) | Gemini Live session, audio queues, prompt/context, tool dispatch, monitoring, reconnect |
| UI | [ui.py](../../example/ui.py) | PyQt HUD, input, settings, avatar/status, confirmations |
| Actions | [actions](../../example/actions) | bundled local tools and proactive tasks |
| Tool discovery | [action_loader.py](../../example/core/action_loader.py) | scans/validates `TOOL` modules, collisions, dispatch |
| Plugins | [plugin_loader.py](../../example/core/plugin_loader.py), [_template.py](../../example/plugins/_template.py) | drop-in Python plugin discovery and execution |
| Safety | [confirm.py](../../example/core/confirm.py), [undo.py](../../example/core/undo.py) | UI-issued confirmation and in-memory undo |
| Memory | [memory_manager.py](../../example/memory/memory_manager.py) | categorized JSON facts, prompt budget/index, lexical recall |
| Config | [config_manager.py](../../example/memory/config_manager.py), `example/config/api_keys.json` | local settings and plaintext provider key |
| Voice | [wake_word.py](../../example/core/wake_word.py), [echo.py](../../example/core/echo.py), [viseme.py](../../example/core/viseme.py), [audio_devices.py](../../example/core/audio_devices.py) | local wake word, echo guard, lip sync, audio selection |
| Vision | [screens.py](../../example/core/screens.py), [screen_processor.py](../../example/actions/screen_processor.py) | screen capture, monitor mapping, image injection |
| Dashboard | [server.py](../../example/dashboard/server.py) | phone/browser dashboard and WebSocket control |
| Tests | [test_screens_and_safety.py](../../example/core/test_screens_and_safety.py) | monitor-coordinate and honest-send invariants |

## Behaviors To Preserve

Preserve behavior through new tests and contracts, not source movement.

### Human authority

The prototype's confirmation token is issued by the interface, not accepted as a model tool parameter. The production equivalent becomes durable, authenticated, multi-client approval bound to the exact intent and policy version.

### Reversibility

The prototype favors undo for reversible actions and confirmation for irreversible actions. Preserve this product instinct, but make undo receipts durable, scoped, expiring, and explicit about effects that cannot be compensated.

### Self-describing capabilities

`TOOL`/`PLUGIN` metadata drives discovery and prompt capability descriptions. Replace it with canonical JSON Schema tool definitions, effect/risk/scopes, manifests, collision tests, process isolation, and compact capability retrieval.

### Memory budget versus storage

The prototype separates stored JSON memory from the small prompt core and exposes on-demand recall with an index. Preserve the separation. Replace JSON categories with sourced typed memory, hybrid retrieval, workspace isolation, correction, and deletion.

### Local audio privacy

Wake word and push-to-talk prevent continuous cloud audio while asleep. Preserve local gating, visible state, explicit device choice, echo handling, and honest platform limits.

### Visual coordinate integrity

Screen-derived coordinates are interpreted against the exact captured monitor/region, including negative origins and resizing. Preserve source labels, geometry transforms, bounds checks, and a screenshot immediately tied to the action. Add policy and user control before interaction.

### Honest outcomes

The current regression tests distinguish "submitted" from confirmed delivery and reject circular success checks. This becomes a universal tool-outcome invariant.

### Cross-platform truthfulness

The example adapts reminders, app launching, settings, and input by OS and reports weaker fallbacks. Preserve capability detection and explicit limitations rather than presenting false parity.

## Behaviors To Redesign

| Current limitation | Production replacement |
| --- | --- |
| One large in-process session owns most behavior | `jarvisd`, application use cases, domain state machines, isolated adapters |
| Gemini-specific reasoning/audio core | independent runtime, model, STT, TTS, and voice ports |
| Tools/plugins execute in process with Python authority | canonical policy gateway plus out-of-process MCP/HTTP/WASI/sandbox extensions |
| Plaintext API key JSON | OS keychain/server secret manager and `SecretRef` |
| JSON memory and lexical scan | SQLite/Postgres typed memory, provenance, hybrid indexes |
| In-memory undo/confirmation | durable approval/compensation records and restart recovery |
| UI callback is required for confirmation | channel-independent approval service with authenticated UI/CLI/mobile/voice presentations |
| Runtime session resumption primarily provider-specific | canonical run state plus optional provider/runtime bindings |
| Dashboard is coupled to the Python process | versioned authenticated daemon API used by all clients |
| Proactive polling inside one process | durable events, schedules, workflows, quiet hours, dedupe and budgets |
| Limited standalone regression script | unit, contract, integration, platform, live, and end-to-end suites |

## Clean-Room Mapping

| Prototype concept | Target owner | First proof |
| --- | --- | --- |
| `JarvisLive` loop | `jarvis-application` + native runtime | scripted streamed run state test |
| action/plugin declarations | `jarvis-tools` + manifests | discovery/collision/schema contract test |
| `confirm.py` | core approval state + application service | forged/stale/mutated intent test |
| `undo.py` | tool compensation receipts/workflow | reversible file operation restart test |
| memory manager | `jarvis-memory` + storage | remember/restart/correct/delete acceptance test |
| Gemini client/live session | `jarvis-models` adapter | official-shape stream fixture + live smoke |
| wake word/audio devices | local voice client | device/wake privacy platform tests |
| echo/viseme | reusable voice/UI algorithms after license review or clean-room replacement | audio timing fixtures and visual tests |
| screen geometry | safe computer-use adapter | multi-monitor round-trip/falsification test |
| dashboard | daemon protocol + web client | authenticated reconnect and scope E2E |
| reminders/proactive | scheduler/workflow/event engine | crash/restart and quiet-hour tests |

## Test Cases To Recreate First

1. An exclusive bottom-right coordinate is rejected rather than clicked.
2. Negative-origin monitor coordinates round-trip through resize/capture geometry.
3. A captured image is labelled with its source and geometry.
4. A communication adapter cannot claim delivery from text it generated itself.
5. An irreversible action cannot run when no approval interface/channel is available.
6. A reversible action records enough prior state to compensate, or says it cannot.
7. A tool-name collision is rejected deterministically without disabling unrelated tools.
8. A memory outside the prompt budget remains discoverable without dumping all memory.
9. Wake/sleep/mute state prevents unintended audio egress.
10. A platform capability that is unavailable reports degraded/unsupported, not success.

## Migration Sequence

1. Freeze the prototype except security/data-loss fixes.
2. Capture behavior catalog and sanitized fixtures permitted by license/terms.
3. Build daemon/CLI/storage vertical slice independently.
4. Reimplement text conversation, tools, approvals, and memory with deterministic tests.
5. Reimplement one connector and one local capability through the new policy path.
6. Build local voice client/provider adapters; compare observable behavior.
7. Add desktop UI after the daemon API is stable enough for generated clients.
8. Import user data only through an explicit previewed migration tool; never read prototype secrets automatically.
9. Keep prototype runnable for comparison until replacement acceptance tests pass.
10. Archive/remove it only after license obligations, user migration, and release notes are settled.

## Import Policy

A future `jarvis migrate mark-liv` command may import user-approved non-secret settings and memory. It must:

- show source/destination and a dry-run manifest
- reject/skip API keys and certificates
- map each fact to provenance `legacy_import`
- avoid treating old language observations as standing instructions
- preserve a backup and support rollback before finalization
- report truncated/invalid/duplicate entries
- require explicit user confirmation before writing canonical memory

## Retirement Gate

The example is no longer needed as an active reference only when installation, conversation, safe tools, memory, local voice, visual geometry, proactive workflows, and cross-platform behavior have equivalent or deliberately superseding acceptance evidence in the new platform.