# ADR-0144: One executable: the daemon is `jarvis daemon`

Status: Accepted
Date: 2026-10-09

## Context

A release shipped two programs, `jarvis` (the client) and `jarvisd` (the daemon), which had to stay in the same folder: the client
found the daemon beside itself to start it and to point a login service at it. A person expects to install one file. Copying only
`jarvis` somewhere else broke starting (the daemon was "not found"), and the two could drift to different versions.

## Decision

1. The daemon's code is a library (`apps/jarvisd` has a `lib.rs` exposing `run_blocking`), and the `jarvis` program depends on it:
   `jarvis daemon [--root DIR] [--version]` runs the daemon, on its own multi-threaded runtime, before any client code starts.
2. `jarvis start` starts **itself** as `jarvis daemon` (`current_exe()`), so there is nothing to find and the daemon's version is
   always the client's. The login service (systemd, launchd, the Windows launcher) runs `<jarvis> daemon`.
3. Release archives and `jarvis path install` carry the single `jarvis` program. A `jarvisd` link left by an older release is still
   handled. The `jarvisd` binary stays as a thin wrapper over the same library, for scripts and tests that start it by name.
4. The trust boundary is unchanged: the client mode still never opens the database, and the daemon mode does what it did. They
   share an executable, not state: the client talks to the daemon over its API as before. The cost is a larger file (the client
   now includes the daemon's code).

## Consequences

Install is one file; `jarvis path install`, `jarvis service install`, and the release smoke test (`jarvis daemon --version`)
follow. A service definition written by an older release (it launches `jarvisd`) is reported as drifted by `jarvis doctor`
and replaced by `jarvis service install`.