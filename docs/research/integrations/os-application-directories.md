---
integration: os-application-directories
status: implemented
last_verified: 2026-09-20
owners: []
selected_spec_version: XDG Base Directory 0.8 / current Windows and macOS platform guidance
selected_sdk: etcetera 0.11.0 plus Rust standard library and native Windows commands
---

# OS Application Directories

## Scope

This record covers Phase 1 resolution and creation of per-user JARVIS config, data, cache, state, runtime, and log directories on Linux, macOS, and Windows. It does not cover service installation, keychains, or IPC creation. Portable-mode overrides (an explicit `--root`) are now implemented and recorded in [portable-and-service-lifecycle.md](portable-and-service-lifecycle.md); this record still owns the native resolution rules those overrides bypass.

## Official Sources

| Source | URL/version | Accessed | Purpose |
| --- | --- | --- | --- |
| AI documentation indexes | Freedesktop, Microsoft Learn, Apple Developer, and docs.rs `llms.txt` paths returned no usable index | 2026-09-20 | discovery attempt |
| XDG Base Directory specification | https://specifications.freedesktop.org/basedir-spec/latest/ (0.8) | 2026-09-20 | Linux config/data/state/cache/runtime paths and modes |
| Microsoft application-data guidance | https://learn.microsoft.com/en-us/windows/apps/develop/data/store-and-retrieve-app-data | 2026-09-20 | per-user local versus roaming storage semantics |
| Microsoft `icacls` reference | https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/icacls | 2026-09-20 | DACL reset, grants, inheritance, numeric SID syntax |
| Microsoft `whoami` reference | https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/whoami | 2026-09-20 | current security principal discovery |
| Apple file-system guidance | https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/FileSystemProgrammingGuide/MacOSXDirectories/MacOSXDirectories.html | 2026-09-20 | Application Support, Caches, and Logs conventions |
| Apple `FileManager` directory API | https://developer.apple.com/documentation/foundation/filemanager/searchpathdirectory | 2026-09-20 | user-domain standard-directory authority |
| `etcetera` package metadata | https://crates.io/api/v1/crates/etcetera (0.11.0) | 2026-09-20 | version, `MIT OR Apache-2.0` license, Rust 1.87 MSRV |
| `etcetera::BaseStrategy` rustdoc | https://docs.rs/etcetera/0.11.0/etcetera/base_strategy/trait.BaseStrategy.html | 2026-09-20 | config/data/cache/state/runtime contract |
| `etcetera` native strategy rustdoc | https://docs.rs/etcetera/0.11.0/etcetera/base_strategy/ | 2026-09-20 | XDG, Apple, and Windows mappings |

The selected crate has only `cfg-if` and `windows-sys` dependencies. Current versioned rustdoc and package metadata are the selected crate authority.

## Verified Contract

### Paths

`etcetera` supplies explicit XDG, Apple, and Windows base strategies. The Windows strategy uses `SHGetKnownFolderPath` when the relevant environment values are absent. The Apple strategy exposes Preferences, Application Support, and Caches; JARVIS intentionally uses Application Support for both config and data to match the architecture contract. The XDG strategy ignores relative environment values. `state_dir` and `runtime_dir` are XDG-only optional paths.

XDG 0.8 requires configured base paths to be absolute and ignores relative values. Its runtime directory must be user-owned, mode `0700`, local, login-lifetime scoped, and suitable for Unix sockets. When it is absent, applications must use a comparable fallback and warn. Newly created application write directories should use mode `0700`.

Windows per-user config uses Roaming AppData; canonical local state stays in Local AppData because Windows 11 no longer supports roaming application data. macOS uses Application Support for config/data, Caches for disposable cache, and Logs for logs.

### Permissions

On Unix, Rust directory builders can create with mode `0700`, and existing JARVIS-owned directories can be tightened to `0700`. Symlink or non-directory endpoints fail closed.

On Windows, `icacls /reset` replaces explicit DACL changes with inherited defaults, `/grant:r` adds/replaces an explicit inheritable full-control ACE for the current `whoami` principal, and `/inheritancelevel:r` disables inheritance while removing inherited ACEs. Arguments are passed directly through `std::process::Command`, never a shell. A live Windows probe on 2026-09-20 began with an explicit `Everyone` full-control ACE plus inherited system, administrator, and user ACEs; the selected sequence ended with only the current user's inheritable full-control ACE.

### Authentication, Limits, And Privacy

No network authentication, pagination, webhooks, retries, or pricing apply. The operating-system account is the authority. Paths can reveal a local account name, so public errors report the path category and operation rather than embedding full paths. Directory preparation is idempotent. Command failure is permanent for the current invocation and must not be reported as a secure directory.

### Versions And Deprecations

`etcetera` 0.11.0 was the current non-yanked release and declares Rust 1.87, below the repository's Rust 1.98.1 pin. Windows roaming data is deprecated on Windows 11, which is why only configuration metadata uses Roaming AppData.

## JARVIS Mapping

- `jarvis-storage` owns local infrastructure path resolution while Phase 1 retains the six-crate architecture.
- Windows/macOS application component: `JARVIS`; Linux component: `jarvis`.
- Config: platform config base plus application component.
- Data: local data base plus application component.
- Cache: platform cache base plus application component, except Windows uses `data/cache`.
- State: XDG state on Linux; local data tree on Windows/macOS.
- Logs: XDG state `logs` on Linux, `~/Library/Logs/JARVIS` on macOS, and local data `logs` on Windows.
- Runtime: XDG runtime on Linux; state/data `runtime` fallback elsewhere or when XDG runtime is absent.
- A runtime-source enum makes the Linux fallback observable to `doctor` and startup diagnostics.

## Decisions

- Use `etcetera` native base strategies, not raw environment-variable concatenation, for OS base locations.
- Use only an explicit caller-provided root for later portable mode; do not silently use the current directory.
- Reject relative native base paths and symlink/non-directory managed endpoints.
- Create and recheck every managed JARVIS directory before use.
- Tighten JARVIS-owned Unix modes to `0700` and Windows DACLs to the current principal.
- Treat inability to secure any required directory as startup failure, not a warning.

## Rejected Alternatives

- `fs-mistrust` 0.15.1 was rejected because its current documentation says Windows ACLs and owners are accepted without inspection.
- `directories` 6.0.0 was rejected at the dependency-policy gate because it transitively requires MPL-2.0 `option-ext`; an equally capable permissive dependency avoids weakening the allowlist.
- `windows-acl` 0.3.0 was rejected because its last release was in 2021 and using it would add old `winapi` surface for behavior available through documented native commands.
- `filp` 0.3.1, `secret-write` 0.1.3, and `clack-private-fs` 1.0.0 were rejected because they were days or weeks old, minimally adopted, or focused on files rather than this directory contract.
- Raw `%APPDATA%`, `%LOCALAPPDATA%`, and `$HOME` concatenation was rejected because it bypasses Known Folder/account lookup and XDG validation.
- Assuming inherited Windows ACLs are private was rejected because a preexisting explicit broad ACE can survive inheritance changes.

## Verification Plan

- Resolve paths and assert the active OS conventions without mutating real application directories.
- Resolve synthetic Linux base roots and reject relative base paths.
- Create all directories under a disposable root and prove repeat preparation is idempotent.
- On Unix, deliberately set a managed directory to `0777`, prepare it, and assert mode `0700`; replace it with a symlink and assert failure.
- On Windows, deliberately grant `Everyone` full control, prepare the directory, and use numeric SID lookup to prove that ACE is absent afterward.
- Make the native permission command fail and prove preparation returns an error rather than success.
- Run the platform tests on native Windows, macOS, and Linux CI.

The cheapest central-contract test is `cargo test -p jarvis-storage paths`; it can disprove deterministic layout, idempotence, and fail-closed permission handling without touching a user's real JARVIS tree.

## Unresolved Questions

- Native macOS permission behavior cannot be claimed from the current Windows host; the same Unix mode implementation must pass on a macOS runner.
- Numeric SID lookup through localized `icacls` output is suitable for test falsification because the known ASCII path is the match signal; production hardening relies only on command status.
- Service-account and machine-wide paths are outside Phase 1 and require separate research.

## Verification Log

| Date | Versions checked | Relevant change or no-change evidence | Researcher |
| --- | --- | --- | --- |
| 2026-09-20 | XDG 0.8, etcetera 0.11.0, current Microsoft/Apple docs | Native Windows broad-ACL removal probe passed; replaced `directories` after cargo-deny rejected its MPL-2.0 transitive dependency | GitHub Copilot |