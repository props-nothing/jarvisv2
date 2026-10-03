# ADR-0128: A container backend confines without FFI, and claims three of five guarantees

**Status:** Accepted
**Date:** 2026-10-03
**Slice:** `P3-020`

## Context

`P3-011` defined the sandbox contracts and implemented one backend. It recorded two limits that together mean
**the platform can enforce nothing on the development host**:

1. On Windows and macOS **no backend exists**, because a job object and a seatbelt profile are `unsafe` FFI and
   this workspace sets `unsafe_code = "forbid"`.
2. The Linux cgroup v2 backend enforces nothing on a host without a delegated cgroup — "a stock install, most
   containers, essentially every interactive development shell" — and the development host is Windows.

`P3-020` asks for "a restricted-execution sandbox backend behind the `P3-011` contracts (a container or an
equivalent isolated worker), naming the guarantees the backend can actually enforce per `ADR-0041`".

A container runtime is the one facility that reaches those hosts **without FFI**, because it is driven by
spawning a CLI. That is the entire reason this backend is possible, and it is also the reason it must be careful:
every flag is a third party's interface, and every claim about what the kernel enforces is that third party's
claim, not this workspace's.

## Decision

**A `ContainerBackend` in `crates/jarvis-sandbox/src/container.rs` drives a container runtime through
`docker run`, and is probed rather than configured.**

**It claims three of the five guarantees: `TreeTermination`, `ProcessCountCeiling`, and `MemoryCeiling`.** It
**does not** claim `CpuRateCeiling` or `CpuTimeCeiling`, and the reasons are different for each — see below. A
request requiring either is refused, which is the mechanism `P3-011` built: `SandboxPolicy::new` resolves every
requirement against the backend **once, at construction**.

**A floor is a floor in both directions — `Some(0)` is refused too.** Probing found that `--memory=0` is
**accepted** by the daemon and means **unlimited**, not "no memory". Everywhere else in this crate an absent
limit is *absent* rather than zero, and `Limits` says so explicitly; a zero here would not be a small ceiling but
the inversion of the field's meaning. So a memory ceiling is refused unless it is at least the 6 MiB the runtime
accepts, and the refusal names the value.

**The environment boundary is `env -i`, and the first attempt at it was a no-op.** `SandboxRequest::environment`
is documented as the **complete** environment, and the process backend implements that as `env_clear().envs(…)`.
A container launch that passed the request's map as `-e` flags would deliver something wider — probed:
`-e FOO=bar` against `alpine:3` leaves the image's `PATH` and `HOME` in place — so the same request would mean
two different variable sets depending on which backend served it.

The launch therefore wraps the program as `env -i <request pairs> <program> <args>`: `-i` starts from an empty
environment and the pairs are consumed before the command. `env` is the one program POSIX requires at a known
path, and it needs **no shell**, so no image's `/bin/sh` is invoked and no string is ever word-split.

The design before it was `/bin/sh -c 'export -n -p; …'` and it was recorded as working before being run.
BusyBox `ash` ignores `export -n` and `-p` only *prints* the variables, so the image's environment survived
intact; in `dash` it would have worked, making the confinement a property of the image rather than of the
backend. The same argv also dropped the program, because `shift` was used to skip a marker that `"$@"` had
already excluded. Both are recorded in the research note, and the reason to record them is that **the design
looked correct and its failure was invisible**: the container started, exited 0, and ran the program.

**`MemoryCeiling` has a floor, and a request below it is refused rather than raised.** The documentation states
it flatly: "If you set this option, the minimum allowed value is `6m` (6 megabytes)." A 1 MiB ceiling and a 6 MiB
ceiling are different policies. Raising one to the other produces a sandbox **stronger** than asked, which is
more tempting to accept than a weaker one and is refused for the same reason: the caller declared a requirement
and then stopped checking.

**The kill path removes the container, and that is why the container has a generated name.**
`LaunchedProcess::kill` implemented as `Child::kill` on the `docker` CLI would leave the **container** running —
the CLI is a client and the work happens in the daemon — so the sandbox would be a process that outlives the
value claiming to own it. `kill` runs `docker rm -f <name>`, which stops the container by removing it, and then
reaps the CLI. The name is therefore a handle rather than a label: it must be unique per launch, or one launch's
kill would stop another's container.

**The image comes from the request, because a container cannot run without one.** `SandboxRequest` gained an
`image: Option<String>`, and it is **optional** rather than required: a container request carries it, a request
for the process backend must not, and a requirement that only one backend can satisfy belongs to that backend.
A **container** launch without one is refused by name; it does not fall back to the `program` field, because a
program path on the host is not an image name and a fallback would make the two interchangeable.

**No image is pulled.** `--pull never` is passed, so a launch refuses when the image is absent. Pulling is a
network fetch of content an operator did not choose, and a sandbox that reaches a registry on demand is a
sandbox whose contents nobody reviewed.

**The backend is selected by probing, never by `cfg!`.** `backend_for_host` asks the runtime whether it can run a
container, because "Docker Desktop is installed" and "the daemon is reachable" are different facts and only the
second is the one that matters. On this development host the probe succeeds where the compiled-target answer is
"nothing".

**What it deliberately does not do.** `--network none`, `--read-only`, `--cap-drop ALL`, and `-i` are all passed
because a confined worker needs them, and **none of them is a `Guarantee`**. They are part of what "a container
isolation" means rather than a policy a caller selects, and the crate models resources rather than a filesystem
or network policy. `--security-opt no-new-privileges` is deliberately **not** passed: it is real hardening, but
it is unmodelled, and enforcing something a caller never asked for makes the sandbox's behaviour a function of a
flag they cannot see.

**The injected launcher no longer builds the process, and this is the change `P3-011`'s seam needed.**
`ProcessLauncher` was a closure that started `request.program` itself, which put two authorities over one process:
the caller's launcher chose the executable and environment, the backend chose the confinement. `P3-020`'s
acceptance test found three failures from that split at once, none of which any unit test could see:

1. A container request's `program` names something **inside the image**, so the launcher spawned a path that is
   not on this host and failed with a bare `entity not found`.
2. The container argv then had to be rebuilt inside the launcher, so the **name generator ran twice** and the
   container was launched under one name while `kill` removed another. The leftover sat in state `Created` — so
   every "is it gone" check that used `docker ps` passed, while the kill reported success.
3. `--name` is the handle the kill path holds, so it can only be owned by whoever builds the argv **once**.

`ProcessLauncher` is now `Box<dyn FnOnce(Command) -> io::Result<Child>>` and the public constructor is
`stdio_launcher(piped)`: a caller decides only whether to capture the standard streams, which is the one thing a
request cannot state because it is a property of how the caller *uses* the child. The ordering property is
unchanged — the command is built **after** the confinement exists and handed to the launcher to spawn, so
`refusing_launcher` still proves that a refusal created no process.

**A request carrying an `image` is refused by the host-process backends, by name.** Without that the pairing is
not merely ineffective but silently wrong: a cgroup backend given an image-bearing request would confine the
`docker` **CLI** with `pids.max`, while the container's own processes live in the daemon's cgroup — so the limits
would bind a client, the confined worker would run unbounded, and `doctor` would report four guarantees. The
container backend refuses the complement (a request **without** an image) for the mirror-image reason: a host
path is not an image reference, and substituting one would make them interchangeable.

**`LaunchedProcess` gained `wait`.** It had `pid()` and `kill()` — a handle that could be destroyed but never
read — so **no** live property of **any** backend was assertable, including "did the child exit, and how". A
supervisor's two questions are "is it done?" and "was it fine?", and neither was answerable. The method consumes
`self` because the future is `'static`; the limitation that follows (a consuming wait cannot be cancelled, so
"wait with a timeout, then kill" is not expressible) is recorded on the trait rather than hidden.

## Consequences

- **A third `Support` value.** `Support::Container`, alongside `CgroupV2` and `Unconfined`, so `doctor` names
  *what* confined a child rather than only that something did.
- **`backend_for_host` now prefers the container when one is available.** The comparison is exact: where the
  cgroup backend works, it confines the *host process* with no image and no runtime dependency; the container
  backend buys guarantee coverage on hosts that otherwise have none. On Windows and macOS the cgroup answer is
  empty, so a probed container wins by default.
- **`SandboxRequest` gained `image`, and the `Default`-shaped construction sites were updated.** It is one
  optional field on a request that already carries seven. It also **selects the backend**, so exactly one
  backend can serve any request and each refuses the other's kind by name — see the Decision for why a wrong
  pairing is silently ineffective rather than merely refused.
- **`ProcessLauncher` changed shape, and `launcher_for` became `stdio_launcher(piped)`.** A caller now decides
  only whether to capture the standard streams; the backend builds the command. This is a breaking change to a
  public seam, and it is the *correction* of one rather than a refactor: the old shape let a caller choose the
  executable while the backend chose the confinement, and the three failures that produced are in the Decision.
  `host_command` is public alongside it, because a caller that wants to *inspect* the command it would get — as
  the host-process tests now do — needs it to be.
- **`LaunchedProcess` gained `wait`.** Its absence meant no live property of any backend could be asserted, so
  the container acceptance test could not have been written at all. It is a breaking change to a public trait
  with three implementors in this crate and none outside it.
- **Recorded as a limit: a daemon that ignores `--pids-limit` is undetectable.** The flag expresses the
  guarantee and the documentation describes it, but whether a given daemon enforces it depends on its cgroup
  driver and kernel support (`docker info` warns when a capability is disabled). A weaker-than-asked outcome
  from there is the one failure this crate cannot see, and it is recorded rather than claimed as covered.
- **Recorded as a limit: the container's own `--network none` is not a guarantee.** `Isolation::Restricted`
  bounds resources. A container with no network is stronger than a cgroup with no network policy, but that is a
  property of the backend rather than of the request, so a caller cannot require it and `doctor` does not report
  it. The same holds for `--read-only` and `--cap-drop ALL`, so **a container is confined more than its
  guarantee list says** — the reverse of the failure this crate exists to prevent, and still a gap between what
  is enforced and what is claimable.
- **Recorded as a limit: the container name is per-process unique, not per-host.** The counter is monotonic
  within a process and the pid disambiguates across processes, which is the same scheme the cgroup leaf uses. Two
  daemons that somehow shared a pid and a counter would collide on `--name`; the launch would fail rather than
  silently adopt the other's container, because the runtime rejects a duplicate name.
- **Unchanged:** the Windows job-object and macOS seatbelt backends still do not exist. A container backend is
  *an* answer for those hosts, not the same answer: it needs a runtime and an image, which a job object does not.
- **Unchanged:** `connect_stdio` in `jarvis-mcp-transport` is still not wired to this crate. `P3-020` supplies
  the backend; the trait seam that would let the MCP transport use it is a separate slice, and it is named here
  rather than implied.
