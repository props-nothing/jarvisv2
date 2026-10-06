//! `jarvis service install` and `jarvis service uninstall`: run JARVIS as a per-user service (`P9-018`).
//!
//! The definition (a systemd unit, a launchd agent, a Windows logon entry) is written to the plan's per-user path and the
//! platform manager is told about it with a fixed list of commands (`ServicePlan::install_steps`). A definition this
//! program did not write is never replaced or removed: ownership is decided by the marker inside it.

use std::process::{Command, Stdio};

use jarvis_diagnostics::{
    OWNERSHIP_MARKER, ServiceCommand, ServiceDrift, ServicePlan, detect_drift,
};

use crate::output::ExitStatus;

fn run(step: &ServiceCommand) -> Result<(), String> {
    println!("  - {}", step.purpose);
    let output = Command::new(&step.program)
        .args(&step.args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("{} could not be run: {error}", step.program))?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    Err(format!(
        "{} {} failed: {}",
        step.program,
        step.args.join(" "),
        detail.trim()
    ))
}

/// `jarvis service install`
pub fn install(plan: &ServicePlan) -> ExitStatus {
    match detect_drift(plan) {
        ServiceDrift::Foreign { path } => {
            eprintln!(
                "jarvis: {} exists and was not written by JARVIS, so it was left alone",
                path.display()
            );
            return ExitStatus::Rejected;
        }
        ServiceDrift::Unreadable { detail } => {
            eprintln!("jarvis: the existing service definition could not be read: {detail}");
            return ExitStatus::Rejected;
        }
        ServiceDrift::Absent | ServiceDrift::Current | ServiceDrift::Drifted { .. } => {}
    }
    if let Some(parent) = plan.definition_path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        eprintln!("jarvis: {} could not be created: {error}", parent.display());
        return ExitStatus::Internal;
    }
    if let Err(error) = std::fs::write(&plan.definition_path, plan.render()) {
        eprintln!("jarvis: the service definition could not be written: {error}");
        return ExitStatus::Internal;
    }
    println!("wrote {}", plan.definition_path.display());
    for step in plan.install_steps() {
        if let Err(message) = run(&step) {
            if step.tolerate_failure {
                println!("    (skipped: {message})");
            } else {
                eprintln!("jarvis: {message}");
                eprintln!(
                    "jarvis: the definition was written; fix the problem above and run `jarvis service install` again"
                );
                return ExitStatus::Unavailable;
            }
        }
    }
    if cfg!(windows) {
        println!("installed: JARVIS starts when you log in (start it now with `jarvis start`).");
    } else {
        println!("installed: JARVIS starts at login and is restarted if it stops.");
    }
    ExitStatus::Ok
}

/// `jarvis service uninstall`
pub fn uninstall(plan: &ServicePlan) -> ExitStatus {
    match detect_drift(plan) {
        ServiceDrift::Absent => {
            // The Windows entry lives in the registry rather than the file, so it is still removed.
            if !cfg!(windows) {
                println!("not installed");
                return ExitStatus::Ok;
            }
        }
        ServiceDrift::Foreign { path } => {
            eprintln!(
                "jarvis: {} was not written by JARVIS (it has no {OWNERSHIP_MARKER} marker), so it was left alone",
                path.display()
            );
            return ExitStatus::Rejected;
        }
        ServiceDrift::Unreadable { detail } => {
            eprintln!("jarvis: the service definition could not be read: {detail}");
            return ExitStatus::Rejected;
        }
        ServiceDrift::Current | ServiceDrift::Drifted { .. } => {}
    }
    for step in plan.uninstall_steps() {
        if let Err(message) = run(&step) {
            println!("    (skipped: {message})");
        }
    }
    if plan.definition_path.exists()
        && let Err(error) = std::fs::remove_file(&plan.definition_path)
    {
        eprintln!(
            "jarvis: {} could not be removed: {error}",
            plan.definition_path.display()
        );
        return ExitStatus::Internal;
    }
    for step in plan.after_uninstall_steps() {
        let _ = run(&step);
    }
    println!(
        "uninstalled: JARVIS no longer starts at login (a running copy keeps running; stop it with `jarvis stop`)."
    );
    ExitStatus::Ok
}
