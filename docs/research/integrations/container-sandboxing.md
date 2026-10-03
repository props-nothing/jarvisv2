# Container sandboxing: Docker CLI as a confinement backend (`P3-020`)

**Access date:** 2026-10-03
**Vendor:** Docker Inc.
**Version observed:** Docker CLI and Engine **29.7.2** (`docker version --format "{{.Server.Version}}"`)
**Local images at probe time:** `rust:1.98-slim-bookworm`, `postgres:18.6-bookworm`, `jarvis:local`, and others.

## Why this record exists

`P3-011` recorded a limit it could not close: **on Windows and macOS no backend exists**, because every job-object
and seatbelt call is `unsafe` FFI and this workspace sets `unsafe_code = "forbid"`. Its second limit was that the
Linux cgroup backend enforces nothing on a host without a delegated cgroup — "a stock install, most containers,
essentially every interactive development shell" — and the development host is Windows, so **no guarantee is
available here at all**.

A container runtime is the one facility that reaches those hosts *without* FFI, because it is driven by spawning
a CLI. That is the whole reason this backend exists, and it is also why it must be researched rather than
remembered: the flags are a third party's interface, and every one of them is a claim about what the kernel will
enforce.

## Sources (official only)

| Source | URL |
| --- | --- |
| Resource constraints | <https://docs.docker.com/engine/containers/resource_constraints/> |
| `docker container run` | <https://docs.docker.com/reference/cli/docker/container/run/> |
| `docker container kill` | <https://docs.docker.com/reference/cli/docker/container/kill/> |

## What each `Guarantee` maps to, and what the documentation actually says

| Guarantee | Flag | Documentation evidence |
| --- | --- | --- |
| `ProcessCountCeiling` | `--pids-limit` | "Tune container pids limit (set -1 for unlimited)". **Container-scoped**, unlike `--ulimit nproc`. |
| `MemoryCeiling` | `--memory` | "The maximum amount of memory the container can use." **"If you set this option, the minimum allowed value is `6m` (6 megabytes). That is, you must set the value to at least 6 megabytes."** |
| `CpuRateCeiling` | `--cpus`, or `--cpu-period` + `--cpu-quota` | "Specify how much of the available CPU resources a container can use… This is the equivalent of setting `--cpu-period='100000'` and `--cpu-quota='150000'`." |
| `TreeTermination` | `docker rm -f` | `docker kill` sends `SIGKILL` to the container's main process. For the **tree**, removing the container stops everything in it. |
| `CpuTimeCeiling` | `--ulimit cpu` | Listed as "CPU time limit in seconds (`RLIMIT_CPU`)". **Per process**, and Docker's own note on the `nproc` option applies to the same class of flag — see below. |

### The two findings that changed the implementation

**1. `--memory` has a 6 MiB floor, and the policy must respect it rather than discover it at launch.**
The sentence is unambiguous — "you must set the value to at least 6 megabytes" — and it means a request whose
ceiling is smaller cannot be expressed as a container limit. The backend therefore **refuses** such a request by
name instead of truncating it upwards: 1 MiB and 6 MiB are different policies, and silently raising one to the
other is the "applied but weaker than asked" outcome `P3-011` forbids. It is stronger than asked rather than
weaker, which makes it *more* tempting to accept and exactly why it is refused.

**A floor is also a floor in the other direction, and the probe found that too.** `--memory=0` is **accepted**
(exit 0) and means *unlimited*, not "no memory": zero reaches the daemon as "no limit set". So `Some(0)` must be
refused as well as `Some(1)`. Everywhere else in this crate an absent limit is *absent* rather than zero —
[`Limits`] documents that explicitly — and a zero arriving here would invert the meaning of the field rather than
merely being too small. Found by probing the boundary rather than by reasoning about it.

**2. `--ulimit nproc` and `--ulimit cpu` are process/user-scoped, not container-scoped.**
Docker's own note on `nproc` is the proof and is worth quoting because it is the same argument `P3-011` made
about `RLIMIT_NPROC`:

> Be careful setting `nproc` with the `ulimit` flag as Linux uses `nproc` to set the maximum number of processes
> available to a user, not to a container… This fails because the caller set `nproc=3` resulting in the first
> three containers using up the three processes quota set for the `daemon` user.

Four containers under one uid share one budget — which is precisely the reasoning that made `P3-011` reject
`RLIMIT_NPROC` as a fallback. **`--ulimit cpu` is the same shape**: `RLIMIT_CPU` is per process, so a tree of N
processes each gets its own budget. So this backend does **not** claim `CpuTimeCeiling` on `--ulimit cpu`, even
though the flag exists and would be easy to pass. `--pids-limit` is claimed because the container's own pid
namespace + cgroup makes it container-scoped.

This is the same asymmetry `P3-011` recorded from the other side: the Linux cgroup backend cannot claim
`CpuTimeCeiling` because cgroup v2 has no cumulative limit file, and a Windows job object could. The three
backends now disagree about CPU-time in three different ways, and each disagreement is stated.

**3. `--env` ADDS to the image's environment; it does not replace it.**
This is the finding that would have broken the allowlist, and it is the sharpest one in this record.
`SandboxRequest::environment` is documented as "the **complete** environment the child receives", and
[`launcher_for`](crate::launcher_for) implements that as `env_clear().envs(...)`. A container launch that passed
the request's map as `-e` flags would deliver something **wider**:

```text
$ docker run --rm -e FOO=bar alpine:3 env
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
HOSTNAME=d5dca6414f7d
FOO=bar
HOME=/root
```

`PATH` and `HOME` come from the image and survive. So the same request would mean two different sets of variables
depending on which backend served it — the divergence `AGENTS.md` forbids by name ("an adapter may not depend on
another adapter") showing up as a *behaviour* difference rather than a dependency one.

The launch therefore passes the image's entrypoint through **`env -i`**, which needs no shell:

```text
env -i            -- clears the environment
FOO=bar BAR=qux   -- the request's pairs, as argv element(s)
<program> <args>  -- the rest of the argv vector
```

`env` is the one program POSIX requires to be at a known path, and `-i` is the flag that means "start with an
empty environment"; the pairs are parsed and everything after them is the command. Verified against `alpine:3`:

| Probe | Result |
| --- | --- |
| `env -i FOO=baz BAR=qux env` after `-e FOO=bar` | `FOO=baz`, `BAR=qux` and **nothing else** — the image's `PATH`/`HOME`/`HOSTNAME` are gone |
| `env -i env` | empty output — no inherited variable at all |
| `env -i printf '[%s]' 'a b' 'c$d'` | `[a b][c$d]` — argv element boundaries survive, so no shell word-splitting or expansion happens |
| `env -i FOO='v v' printf '[%s]' 'a b'` | `[a b]` — the pairs are consumed and the program follows them |
| `env -i /bin/sh -c 'exit 7'` | exit **7**, so a status propagates through the wrapper |

**The first attempt was a shell wrapper and was silently a no-op — recorded because it passed every check
except the useful one.** The design was `/bin/sh -c 'export -n -p; …; exec "$@"'`, on the belief that
`export -n -p` clears the environment in a POSIX shell. Probed rather than assumed, BusyBox `ash` printed
`export FOO='bar'` / `export HOME='/root'` / … — `export -n` **ignored**, `-p` merely printed the variables,
and the image's environment intact. In `dash` it would have worked, so the same argv would confine correctly on
one image and not on another. `-p` means *print*, and that is the whole of the bug. A second error hid in the
same design: `shift; exec "$@"` treats `$0` as the marker and drops it, but `"$@"` **already excludes** `$0`, so
the shift discarded the program instead — the probe found it as `exec: line 0: a: not found`. Both are recorded
because both looked correct, and neither is detectable without running the exact argv.



### Flags used, and why

| Flag | Reason |
| --- | --- |
| `--rm` | "Automatically remove the container… when it exits". Without it every launch leaves a stopped container accumulating on the host. |
| `--name <generated>` | **Required by the kill path** — see below. |
| `--network none` | Not a `Guarantee`; a container that can reach the network is not an isolated worker. |
| `--read-only` | "Mount the container's root filesystem as read only prohibiting writes to locations other than the specified volumes". |
| `--cap-drop ALL` | The default already "drops most potentially dangerous kernel capabilities"; this drops all of them. |
| `-i` | `run` with a command and no `-i` closes stdin immediately, so an MCP server reading stdin would see EOF. |

**`--security-opt no-new-privileges` is deliberately not passed.** It is a real hardening step, but it is *not*
one of the five guarantees this crate models, and adding an unmodelled restriction would make the backend
enforce something a caller never asked for and cannot see. Named here as a limit rather than silently omitted.

## The kill path is the part that would have been wrong

`docker kill` sends `SIGKILL` to the container's main process, and a container's PID namespace means killing the
`docker` **CLI** does not stop the container: the CLI is a client, and the run continues in the daemon. A
`LaunchedProcess::kill` implemented as `Child::kill` on the CLI would therefore leave a **running container** —
the failure being a confined process that outlives its confinement, which is worse than no sandbox because the
caller believes it stopped.

So `kill` runs `docker rm -f <name>`, which removes the container and stops it by virtue of removal, and then
reaps the CLI. That is why the container needs a **generated name**: it is the handle the kill path holds, and
it must be unique per launch or one launch's kill would stop another's container.

**Verified: the container must be launched under the name the kill path holds, and the first implementation was
not.** `docker run` creates the container **as** the client starts, so a container exists to be leaked before
`spawn` even returns — a name mismatch therefore leaves it in state `Created`, and `docker ps -a` (not
`docker ps`) is what reveals it, because a `Created` container is not running and every "is it gone" check that
used `docker ps` would pass. The mismatch arose from building the argv twice; the fix is that the backend builds
it **once** and reads the name back out of that same vector. Probed: `docker rm -f N` on a live container leaves
no entry in `docker ps -a` and no `docker` CLI process.

**And `docker ps` is a host-global view, so it cannot be read from parallel tests.** Two live tests in one binary
saw each other's containers, and one read a concurrent launch's absence as a removal that had not happened —
with the leftover's *pid* in the failure message being the evidence that it was not this process's container.
Also probed: the container appears in the list **after** `spawn` returns, so a list taken immediately can
legitimately be empty and must be polled rather than asserted. An in-process `tokio::sync::Mutex` plus a poll is
what the acceptance tests do.

**Verified: `exit 7` through the wrapper `env -i`.** The CLI must propagate the container's status for a launch
to be supervisable, and it does — including when the command is wrapped in `env -i`, so the boundary costs no
fidelity. `status.code()` is asserted as **7** rather than merely non-zero, because a constant `1` for every
failure would satisfy a success/non-success check while losing the information a supervisor needs.

## Verified empirically on this host (not only read)

| Probe | Result |
| --- | --- |
| `docker version --format "{{.Server.Version}}"` | `29.7.2`, exit 0 |
| `docker info --format "{{.ServerVersion}}"` | `29.7.2`, exit 0 — **no trailing warning**, so the probe's stdout is parseable |
| `docker run --rm … sh -c "touch /x"` under `--read-only` | `touch: /x: Read-only file system` — the rootfs really is read-only |
| `docker run --rm alpine:3 sh -c "exit 42"` | shell exit **42**, so `docker run` propagates the container's exit status |
| `docker run … echo hello` | stdout `hello`, exit 0 — output streams through, so a caller sees the child's output |
| `docker run --rm --memory=1m alpine:3 true` | exit **125**, stderr `docker: Error response from daemon: Minimum memory limit allowed is 6MB` — the floor is enforced by the daemon, and the refusal is what a launch would surface |
| `docker run --rm --memory=6m alpine:3 true` | exit **0** — the floor itself is accepted, so the constant is inclusive |
| `docker run --rm --memory=0 alpine:3 true` | exit **0** — **zero means unlimited**, not "none"; see finding 1 |
| `docker run --rm --pids-limit 0 alpine:3 true` | exit **0** — a zero process limit does **not** prevent a container from starting |
| `docker run --rm --name N alpine:3 sleep 120`, then `docker rm -f N` | container `Up 2 seconds` → gone from `docker ps -a`, `rm` exit 0, and **no `docker` CLI process left** — so remove-and-reap leaves nothing behind |

The exit-code probe matters because the backend's `docker` child *is* the process under confinement: a caller
that could not observe the container's status would have a launch it cannot supervise.

## Unresolved / not attempted

- **No image is pulled by this backend.** `--pull never` is passed, so a launch refuses when the image is absent
  rather than fetching it: pulling is a network fetch of untrusted content, and a sandbox that reaches the
  registry on demand is a sandbox whose contents an operator did not choose. Verified absent by reading the
  `--pull` option table: `never` — "Do not pull the image, even if it's missing, and produce an error if the
  image does not exist in the image cache."
- **`--pids-limit` upper bound and the daemon's own cgroup configuration are not verified here.** Whether a
  given daemon enforces the limit depends on its cgroup driver and kernel support, which the documentation
  addresses under the "check for support" section (`docker info` warns when a capability is disabled). The
  backend claims the guarantee because the flag expresses it; a daemon that silently ignores it would be a
  **weaker-than-asked** outcome this crate cannot detect, and that is recorded as a limit rather than claimed as
  covered.
- **Docker Desktop on Windows requires WSL2 or Hyper-V**, so "a container is available" is a property of the
  probe and not of the platform. `probe()` answers by running the daemon, never by reading `cfg!(windows)`.
