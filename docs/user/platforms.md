# Platforms: Windows, Linux (including a headless VPS) and macOS

JARVIS is built to run the same way on all three. This page says what that means today, **how each claim was checked**, how to run it
on a server with no screen, and what is still missing. It is deliberately specific about what has and has not been run.

## Support status

| | Windows 10/11 | Linux (x86_64, aarch64) | macOS (Apple silicon, Intel) |
| --- | --- | --- | --- |
| Compiles, clippy clean with warnings denied | yes | **yes** (CI and a local cross-check) | **yes** (local cross-check; CI) |
| Automated tests | pass locally | run in CI on every push | run in CI on every push |
| Run end to end by a person | **yes**, throughout development | not yet | not yet |
| Pure-Rust TLS (no OpenSSL to install) | yes | yes | yes |
| Native config/data/log locations | `%APPDATA%`/`%LOCALAPPDATA%` | XDG (`~/.config`, `~/.local/share`) | `~/Library/Application Support` |
| Private key and credential files | owner-only ACL | mode `0600` | mode `0600` |
| Run at login / as a service | registry Run entry (`jarvis service install`) | systemd user unit (`jarvis service install`) | launchd agent (`jarvis service install`) |
| Code sandbox (optional) | Docker Desktop | Docker | Docker Desktop |
| Prebuilt archive | CI builds a private artifact (`release.yml`) | CI builds a private artifact | CI builds a private artifact (Apple silicon and Intel) |
| Signed installers, public downloads | not yet | not yet | not yet (an unsigned binary needs `xattr -d com.apple.quarantine` or a notarized build, `P9-005`) |

"Run end to end by a person" is the honest gap: the Linux and macOS code paths compile and lint cleanly and their tests run in CI, but
nobody has used JARVIS on a real Linux box or Mac yet. Treat the first run there as a test and report what you find (`P9-022`).

The first release workflow run built all five archives on GitHub; its smoke test (unpack, run `jarvis version` and `jarvisd --version`) failed
on every platform because of a shell glob in the script itself, since fixed and verified by packaging and running the Windows archive locally.
macOS is the one CI job still red: the Phase 3 gate (a real daemon and an MCP child process) fails there for a reason not yet read.

What running the tests on real Linux and macOS runners has already found (and fixed): a socket path longer than macOS allows
(103 bytes) failed at bind with a message that named nothing, so JARVIS now refuses it by name and says to use `--root` with a short
directory; a sandbox test that assumed a delegated cgroup on any Linux host; and three tests that used Windows-style paths. None of
these was a defect on Windows, which is why only a second platform finds them.

How the first two rows are checked: CI (`.github/workflows/ci.yml`) runs formatting, clippy with warnings denied on all targets, and the
whole test suite on Ubuntu, macOS and Windows; and from a Windows machine the same clippy gate can be run against the Linux and macOS
targets by pointing the C compiler at a stub (the C dependencies are only compiled, never linked, by `cargo check`).

## A Linux server (a VPS with no screen)

JARVIS listens **only on `127.0.0.1`**. That is deliberate (an exposed assistant is an attack surface) and it means the console is
reached through an SSH tunnel, not a public address. A remote, TLS-terminated mode is a separate, explicit piece of work (`P10-004`);
until it exists, **do not forward the port to the internet**.

```text
# on the server
jarvis init --base-url https://api.example.com/v1 --model NAME --api-key-file /home/me/model.key --workspace /home/me/work
jarvis start                  # starts in the background and keeps running after you log out
jarvis service install        # starts at boot-time login and restarts if it stops (also enables lingering)
jarvis status
jarvis doctor --live          # is the model reachable and the key accepted? is a folder missing? each finding says how to fix it

# on your own computer
ssh -L 8765:127.0.0.1:8765 me@your-server
#   then on the server: jarvis hud --print-url      (prints the address with your credential)
#   and open that address in your own browser
```

Notes:

- With no display, `jarvis` and `jarvis hud` print the tunnel instructions instead of failing to open a browser.
- The voice (microphone, spoken answers) runs in the browser on your computer, not on the server, so it works through the tunnel.
- A local Ollama on the server works the same as on a desktop; a hosted model needs `--api-key-file`.
- `jarvis service install` writes `~/.config/systemd/user/jarvisd.service` and runs `systemctl --user enable --now`, then
  `loginctl enable-linger` so the service survives the last SSH session ending. If lingering is refused on your host, ask the
  administrator to enable it for your user.
- Only the default profile can be installed as a service; a `--root` portable profile deliberately cannot.

## macOS

- `jarvis` opens the console with `open`. `jarvis service install` writes a launchd agent
  (`~/Library/LaunchAgents/dev.jarvis.jarvisd.plist`) and loads it.
- A downloaded, unsigned build is quarantined by Gatekeeper; until signed and notarized builds exist (`P9-005`), build from source or
  remove the quarantine attribute from both programs.

## Windows

`jarvis service install` adds a per-user Run entry (no administrator rights) that starts the daemon hidden at login; `jarvis service
uninstall` removes it. `jarvis start` launches the daemon so that scripts which capture its output do not hang.

## Known gaps

- Archives are built by CI (Actions, "Release (private artifacts)"). Run it by hand for 14-day workflow artifacts, or push a `v*` tag to
  also attach the archives to a **draft** GitHub release, which only people with write access can see. Nothing is published, because no
  licence has been chosen yet, and there is no installer, signing or update mechanism (`P9-004` to `P9-007`, `P9-017b`). Each archive has
  `jarvis` and `jarvisd` side by side plus a SHA-256 file; unpack it and run `jarvis`. This workflow has not run on GitHub yet, so the first
  run (`Run workflow`) is also its test.
- The Linux sandbox backend without a container runtime has no wait-for-exit support recorded in the sandbox research; use the
  Docker-backed code tool.
- Telephony, messaging apps and the connectors are not built, so a server has no way to reach you except the console, the CLI and the
  schedule runner (`ROADMAP.md`).
- Several optional integrations (a coding-agent runtime, local speech) have not been designed for a headless host yet.