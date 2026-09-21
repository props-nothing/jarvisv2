use std::sync::atomic::{AtomicU8, Ordering};

use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum Phase {
    Booting = 0,
    Ready = 1,
    Stopping = 2,
    Stopped = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HealthSnapshot {
    pub(crate) live: bool,
    pub(crate) ready: bool,
    pub(crate) phase: &'static str,
}

#[derive(Debug, Error)]
#[error("invalid daemon lifecycle transition from {from} to {to}")]
pub(crate) struct LifecycleError {
    from: &'static str,
    to: &'static str,
}

#[derive(Debug)]
pub(crate) struct HealthState {
    phase: AtomicU8,
}

impl HealthState {
    pub(crate) const fn new() -> Self {
        Self {
            phase: AtomicU8::new(Phase::Booting as u8),
        }
    }

    pub(crate) fn snapshot(&self) -> HealthSnapshot {
        let phase = decode(self.phase.load(Ordering::Acquire));
        HealthSnapshot {
            live: phase != Phase::Stopped,
            ready: phase == Phase::Ready,
            phase: phase_name(phase),
        }
    }

    pub(crate) fn mark_ready(&self) -> Result<(), LifecycleError> {
        self.transition(Phase::Booting, Phase::Ready)
    }

    pub(crate) fn begin_shutdown(&self) -> Result<(), LifecycleError> {
        self.transition(Phase::Ready, Phase::Stopping)
    }

    pub(crate) fn mark_stopped(&self) -> Result<(), LifecycleError> {
        self.transition(Phase::Stopping, Phase::Stopped)
    }

    fn transition(&self, from: Phase, to: Phase) -> Result<(), LifecycleError> {
        self.phase
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|actual| LifecycleError {
                from: phase_name(decode(actual)),
                to: phase_name(to),
            })
    }
}

const fn decode(value: u8) -> Phase {
    match value {
        0 => Phase::Booting,
        1 => Phase::Ready,
        2 => Phase::Stopping,
        _ => Phase::Stopped,
    }
}

const fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Booting => "booting",
        Phase::Ready => "ready",
        Phase::Stopping => "stopping",
        Phase::Stopped => "stopped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_transitions_make_readiness_and_liveness_explicit() {
        let health = HealthState::new();
        assert_eq!(
            health.snapshot(),
            HealthSnapshot {
                live: true,
                ready: false,
                phase: "booting"
            }
        );

        assert!(health.mark_ready().is_ok());
        assert!(health.snapshot().ready);
        assert!(health.begin_shutdown().is_ok());
        assert_eq!(health.snapshot().phase, "stopping");
        assert!(!health.snapshot().ready);
        assert!(health.mark_stopped().is_ok());
        assert!(!health.snapshot().live);
    }

    #[test]
    fn duplicate_or_out_of_order_health_transitions_fail() {
        let health = HealthState::new();
        assert!(health.begin_shutdown().is_err());
        assert!(health.mark_ready().is_ok());
        assert!(health.mark_ready().is_err());
        assert!(health.mark_stopped().is_err());
    }
}
