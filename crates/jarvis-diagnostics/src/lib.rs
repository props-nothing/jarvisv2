//! Offline diagnostics and `jarvis doctor` checks for JARVIS.
//!
//! Doctor answers "why is this installation not working?" without changing it.
//! It separates detection, evidence, repair authority, and post-repair
//! verification so a diagnosis is never confused with a repair.
//!
//! It depends on adapters (`jarvis-storage`, `jarvis-observability`, and
//! `jarvis-protocol`) because a finding must cite real state. `jarvis-cli` cannot
//! own this code: the CLI is forbidden from touching the database, yet doctor
//! must inspect it offline, before a daemon exists.

mod doctor;
mod findings;
mod repair;
mod service;

pub use doctor::{
    CheckResult, DoctorError, Outcome, PathEvidence, Report, ReportOutcome, diagnose,
    runtime_source_name,
};
pub use findings::{Finding, FindingCode, Severity};
pub use repair::{RepairOutcome, RepairReport, repair};
pub use service::{
    OWNERSHIP_MARKER, ServiceDrift, ServiceError, ServiceKind, ServicePlan, daemon_binary_beside,
    detect_drift, drift_finding,
};
