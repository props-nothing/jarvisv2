//! What a sandbox is asked for, what it guarantees, and why it can refuse.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

/// The strongest confinement a caller may ask for.
///
/// Ordered from least to most confining, so a caller can compare two policies. The names describe what is
/// **enforced**, not what the child is trusted with: an untrusted program under `Restricted` is a program
/// whose *resource* use is bounded, which is a different claim from "it cannot reach the network".
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Isolation {
    /// No OS confinement. The child runs with this process's authority and limits.
    ///
    /// Present as a **value** rather than as the absence of one, so "we did not confine this" is a decision a
    /// caller states and an audit record can name. If it were an `Option::None` instead, an accidental
    /// omission would be indistinguishable from a deliberate choice.
    None,
    /// Resource limits on the child's **tree**: process count, memory, and CPU.
    ///
    /// What is *not* claimed: no network policy, no filesystem confinement, no privilege reduction. A child
    /// under `Restricted` can still open the daemon's files, because none of the primitives available to this
    /// crate express a filesystem view for another process.
    Restricted,
}

/// A guarantee a caller may require, checked against what the platform can actually enforce.
///
/// # Why `CpuRateCeiling` and `CpuTimeCeiling` are two variants and not one
///
/// They are the clearest evidence for this crate's central claim — that a guarantee is a *named* property
/// rather than a boolean "sandboxed" — because the two platforms cannot both express both:
///
/// | | Linux cgroup v2 | Windows job object |
/// | --- | --- | --- |
/// | CPU **rate** (a budget per period) | `cpu.max` = `$MAX $PERIOD` — **yes** | no equivalent |
/// | CPU **time** (a cumulative total) | `cpu.stat` only *accounts*; there is no limit file — **no** | `PerJobUserTimeLimit` — **yes** |
///
/// One variant covering both would force a backend to claim a guarantee it cannot enforce on one of the two
/// platforms, which is exactly the silent degradation this crate exists to prevent. Splitting them makes the
/// Linux backend's support set *provably* exclude `CpuTimeCeiling`, and that exclusion is testable.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Guarantee {
    /// The child's **whole process tree** is killed, including descendants it creates after launch.
    TreeTermination,
    /// A hard ceiling on the number of processes in the tree, enforced by the kernel.
    ///
    /// The distinction that matters: a per-process `RLIMIT_NPROC` is *not* this. A child that reaches its own
    /// limit can still `fork`, because the count is kept **per user id** and every process the child creates
    /// gets its own budget under the same uid. Only a tree-scoped counter makes a fork bomb fail.
    ProcessCountCeiling,
    /// A hard ceiling on memory committed by the tree.
    MemoryCeiling,
    /// A ceiling on the CPU **rate** the tree may consume, expressed as time allowed per period.
    CpuRateCeiling,
    /// A ceiling on the **cumulative** CPU time the tree may consume before it is stopped.
    CpuTimeCeiling,
}

impl Guarantee {
    /// Returns the label used in diagnostics and audit evidence.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TreeTermination => "tree_termination",
            Self::ProcessCountCeiling => "process_count_ceiling",
            Self::MemoryCeiling => "memory_ceiling",
            Self::CpuRateCeiling => "cpu_rate_ceiling",
            Self::CpuTimeCeiling => "cpu_time_ceiling",
        }
    }
}

impl std::fmt::Display for Guarantee {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What to launch, and under what policy.
///
/// The program and its limits live in one struct so a caller cannot build a request whose policy was
/// forgotten: there is one constructor and it takes everything.
#[derive(Clone, Debug)]
pub struct SandboxRequest {
    /// The executable to launch.
    pub program: PathBuf,
    /// Its arguments, passed as a vector rather than a shell string.
    ///
    /// **There is no shell.** `docs/architecture/security.md` lists command injection as a threat with
    /// "structured commands where possible; no host shell by default" as the mitigation, and a vector is that
    /// mitigation made structural: there is no string for a metacharacter to be interpreted in.
    pub arguments: Vec<String>,
    /// The directory the child starts in, if one is chosen.
    pub working_directory: Option<PathBuf>,
    /// The **complete** environment the child receives.
    ///
    /// `security.md` asks for an "environment allowlist", and a map *is* that allowlist: the child is given
    /// exactly these pairs and nothing else, so no host credential, token, or `PATH` entry is inherited by
    /// omission. A list of variable names to *keep* would leave the values unstated; this states them.
    pub environment: BTreeMap<String, String>,
    /// The isolation level being requested.
    pub isolation: Isolation,
    /// The limits the caller wants, if any.
    pub limits: Limits,
    /// The guarantees the caller **requires**, each of which must be enforceable or the launch is refused.
    pub required: Vec<Guarantee>,
}

/// The numeric limits a request carries.
///
/// Every field is optional, and an absent limit is *absent* rather than zero: `Some(0)` for a process ceiling
/// would forbid the child from existing, which is never what a caller means, so the two states are kept apart
/// instead of being conflated by a default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Limits {
    /// Maximum processes in the tree, including the child.
    pub max_processes: Option<u32>,
    /// Maximum memory committed by the tree, in bytes.
    pub max_memory_bytes: Option<u64>,
    /// CPU time the tree may consume **per period**, for [`Guarantee::CpuRateCeiling`].
    pub max_cpu_rate: Option<Duration>,
    /// **Cumulative** CPU time the tree may consume in total, for [`Guarantee::CpuTimeCeiling`].
    ///
    /// Separate from `max_cpu_rate` because it is a different policy, not a different unit: a rate ceiling
    /// throttles a long-running child indefinitely, while a cumulative ceiling stops it once. No Linux backend
    /// can honour this field, so a request that sets it must also require the guarantee — which is what turns
    /// the unenforceable case into a refusal instead of a silently ignored value.
    pub max_cpu_time: Option<Duration>,
}

/// Why a sandbox could not be applied.
///
/// Every variant is a **refusal at launch**. There is no variant for "applied but weaker than asked", because
/// that is the state this crate exists to make impossible: a caller who required a guarantee either gets it or
/// gets this error.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    /// The platform cannot enforce a guarantee the caller required.
    #[error("this platform cannot guarantee {guarantee}")]
    UnsupportedGuarantee {
        /// The guarantee that could not be provided.
        guarantee: Guarantee,
    },
    /// The backend could not be constructed, which names the OS facility that failed.
    ///
    /// The `reason` is a bounded, **non-sensitive** explanation: it names the facility and the failure class,
    /// never a path from the host or a value from the environment.
    #[error("the {facility} sandbox could not be created: {reason}")]
    Setup {
        /// The OS facility, e.g. `"cgroup v2"`.
        facility: &'static str,
        /// A bounded description of what failed.
        reason: String,
    },
    /// The child could not be spawned, or could not be confined after spawning.
    ///
    /// Separate from [`Self::Setup`] because the child may already exist by this point, so the caller's
    /// response is to make sure it is dead rather than to report a configuration problem.
    #[error("the sandboxed process could not be started: {reason}")]
    Launch {
        /// A bounded description of what failed.
        reason: String,
    },
}

impl SandboxError {
    /// Returns the guarantee this error names, so a caller can log which requirement failed.
    ///
    /// `None` for a setup or launch failure, which is about the facility rather than about a named guarantee.
    #[must_use]
    pub const fn unsupported(&self) -> Option<Guarantee> {
        match self {
            Self::UnsupportedGuarantee { guarantee } => Some(*guarantee),
            Self::Setup { .. } | Self::Launch { .. } => None,
        }
    }
}

/// A request whose requirements have been checked, or a refusal.
///
/// The point of a separate type is that **checking happens once, at construction**. A caller who had to
/// remember to call `supports()` before every launch would eventually forget, and the failure would be a child
/// running with less confinement than the caller believed. There is no public constructor other than
/// [`SandboxPolicy::new`].
pub struct SandboxPolicy {
    request: SandboxRequest,
    /// Every guarantee the chosen backend confirmed it can enforce, recorded as evidence for `doctor`.
    supported: Vec<Guarantee>,
}

impl std::fmt::Debug for SandboxPolicy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SandboxPolicy")
            .field("program", &self.request.program)
            .field("isolation", &self.request.isolation)
            .field("required", &self.request.required)
            .field("supported", &self.supported)
            .finish_non_exhaustive()
    }
}

impl SandboxPolicy {
    /// Checks every required guarantee against a backend, or refuses.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxError::UnsupportedGuarantee`] naming the **first** requirement the backend cannot
    /// enforce. First rather than all: a caller fixing its configuration needs one actionable reason, and a
    /// list of every failure would change shape as the platform does.
    pub fn new(
        request: SandboxRequest,
        backend: &dyn crate::SandboxBackend,
    ) -> Result<Self, SandboxError> {
        let support = backend.support();
        for guarantee in &request.required {
            if !support.supports(*guarantee) {
                return Err(SandboxError::UnsupportedGuarantee {
                    guarantee: *guarantee,
                });
            }
        }
        Ok(Self {
            supported: support.guarantees().to_vec(),
            request,
        })
    }

    /// Returns the request this policy was built from.
    #[must_use]
    pub const fn request(&self) -> &SandboxRequest {
        &self.request
    }

    /// Returns every guarantee the backend can enforce, not only the required ones.
    ///
    /// This is the value `doctor` reports. It is the **backend's** full list rather than the request's, because
    /// the question an operator is asking is "what would this machine enforce", which does not depend on which
    /// launch happened to ask.
    #[must_use]
    pub fn supported(&self) -> &[Guarantee] {
        &self.supported
    }

    /// Returns the guarantees in force for this policy's launch.
    ///
    /// Equal to `request.required` by construction: [`Self::new`] refuses unless every required guarantee is in
    /// `supported`, so reaching here is the proof. Returned as its own method rather than read from the request
    /// at the call site, so a launch and an audit record cannot disagree about what was promised — and so a
    /// future relaxation of the check has exactly one place to be wrong.
    #[must_use]
    pub fn enforced(&self) -> Vec<Guarantee> {
        self.request.required.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backend whose support list is whatever the test says, so policy logic is testable with no OS.
    struct StubBackend {
        support: crate::GuaranteeSupport,
    }

    impl crate::SandboxBackend for StubBackend {
        fn support(&self) -> crate::GuaranteeSupport {
            self.support.clone()
        }

        fn facility(&self) -> crate::Support {
            crate::Support::Unconfined
        }

        fn launch(
            &self,
            _policy: &SandboxPolicy,
            _launcher: crate::ProcessLauncher,
        ) -> crate::BackendFuture<Result<crate::Launched, SandboxError>> {
            Box::pin(std::future::ready(Err(SandboxError::Setup {
                facility: "stub",
                reason: "a stub backend does not launch".to_owned(),
            })))
        }
    }

    fn request(required: Vec<Guarantee>) -> SandboxRequest {
        SandboxRequest {
            program: PathBuf::from("/nonexistent/program"),
            arguments: Vec::new(),
            working_directory: None,
            environment: BTreeMap::new(),
            isolation: Isolation::Restricted,
            limits: Limits::default(),
            required,
        }
    }

    fn support(guarantees: &[Guarantee]) -> crate::GuaranteeSupport {
        crate::GuaranteeSupport::from_guarantees(guarantees.iter().copied())
    }

    /// **A requirement the backend cannot enforce is refused, and the refusal names it.**
    ///
    /// `CpuTimeCeiling` is chosen deliberately: it is the guarantee a Linux cgroup-v2 backend genuinely cannot
    /// provide, so this test states a real asymmetry between the platforms rather than an invented one. The
    /// property is the crate's central claim — a policy that degraded to "whatever we can do" would let a
    /// caller believe a fork bomb was bounded when it was not, and by then the caller has stopped checking.
    #[test]
    fn an_unenforceable_requirement_is_refused_by_name() {
        let backend = StubBackend {
            support: support(&[Guarantee::TreeTermination]),
        };
        let error = SandboxPolicy::new(request(vec![Guarantee::CpuTimeCeiling]), &backend)
            .err()
            .unwrap_or_else(|| panic!("a requirement the backend cannot meet must be refused"));
        assert_eq!(error.unsupported(), Some(Guarantee::CpuTimeCeiling));
        assert!(
            error.to_string().contains("cpu_time_ceiling"),
            "the refusal must name the guarantee so an operator can act, got: {error}"
        );
    }

    /// **A requirement the backend can enforce is accepted, and the policy records the backend's full list.**
    ///
    /// The positive control for the test above: without it, a `new` that refused everything would pass.
    /// The recorded list is asserted to be the backend's **whole** set rather than the request's subset,
    /// because that is what `doctor` reports and a subset there would understate the machine.
    #[test]
    fn an_enforceable_requirement_is_accepted_and_the_full_support_is_recorded() {
        let backend = StubBackend {
            support: support(&[Guarantee::TreeTermination, Guarantee::ProcessCountCeiling]),
        };
        let policy = SandboxPolicy::new(request(vec![Guarantee::TreeTermination]), &backend)
            .unwrap_or_else(|error| panic!("an enforceable requirement must be accepted: {error}"));
        assert_eq!(
            policy.supported(),
            &[Guarantee::TreeTermination, Guarantee::ProcessCountCeiling],
            "the recorded support is the machine's, not the request's"
        );
        assert_eq!(
            policy.enforced(),
            vec![Guarantee::TreeTermination],
            "what is in force is what was required, and construction proved each is supported"
        );
    }

    /// **A request with no requirements is accepted on a backend that guarantees nothing.**
    ///
    /// `Isolation::None` has to remain expressible, or an operator cannot configure a server they trust. The
    /// property that matters is that it is a **stated** choice: `required` is empty, so the policy's enforced
    /// set is empty rather than unknown.
    #[test]
    fn a_request_that_requires_nothing_is_accepted_with_empty_support() {
        let backend = StubBackend {
            support: support(&[]),
        };
        let policy = SandboxPolicy::new(request(Vec::new()), &backend).unwrap_or_else(|error| {
            panic!("a request requiring nothing must be accepted: {error}")
        });
        assert!(policy.supported().is_empty());
        assert!(policy.enforced().is_empty());
    }

    /// **The first unenforceable requirement is the one named, deterministically.**
    ///
    /// A caller fixing its configuration needs one actionable reason. This pins that the order is the
    /// request's, so the message does not depend on the order of the backend's set.
    #[test]
    fn the_refusal_names_the_first_requirement_in_the_requests_order() {
        let backend = StubBackend {
            support: support(&[]),
        };
        let error = SandboxPolicy::new(
            request(vec![Guarantee::MemoryCeiling, Guarantee::TreeTermination]),
            &backend,
        )
        .err()
        .unwrap_or_else(|| panic!("nothing is supported, so this must be refused"));
        assert_eq!(
            error.unsupported(),
            Some(Guarantee::MemoryCeiling),
            "the request's own order decides which requirement is named"
        );
    }

    /// **Every guarantee has a distinct, stable label, and the two CPU guarantees differ.**
    ///
    /// A label is audit evidence, so two guarantees sharing one would make a record ambiguous. The CPU pair is
    /// asserted explicitly because collapsing them is the tempting simplification this crate rejects.
    #[test]
    fn guarantee_labels_are_distinct() {
        let all = [
            Guarantee::TreeTermination,
            Guarantee::ProcessCountCeiling,
            Guarantee::MemoryCeiling,
            Guarantee::CpuRateCeiling,
            Guarantee::CpuTimeCeiling,
        ];
        let mut labels: Vec<&str> = all.iter().map(|guarantee| guarantee.as_str()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(
            labels.len(),
            all.len(),
            "labels must be distinct: {labels:?}"
        );
        assert_ne!(
            Guarantee::CpuRateCeiling.as_str(),
            Guarantee::CpuTimeCeiling.as_str(),
            "a rate ceiling and a cumulative ceiling are different promises"
        );
        for guarantee in all {
            assert_eq!(guarantee.to_string(), guarantee.as_str());
        }
    }
}
