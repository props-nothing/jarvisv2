//! `jarvis stop` and `jarvis restart`: end the background daemon gracefully, and start it again.
//!
//! Stopping asks the daemon over its authenticated API (`POST /api/v1/shutdown`), which runs the same orderly shutdown an
//! operating-system signal does, and then waits until the port is free so a following start cannot race it.

use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::output::ExitStatus;

/// How long a stopping daemon is given to release its port.
const STOP_WAIT: Duration = Duration::from_secs(30);

fn listening(address: &SocketAddr) -> bool {
    TcpStream::connect_timeout(address, Duration::from_millis(300)).is_ok()
}

/// What happened when a stop was asked for.
#[derive(Debug, Eq, PartialEq)]
pub enum StopOutcome {
    /// Nothing was listening, so there was nothing to stop.
    NotRunning,
    /// The daemon stopped and its port is free.
    Stopped,
    /// The daemon is still working on tasks that a stop would interrupt, so it was left running. Carries its explanation.
    Busy(String),
}

/// Asks the daemon on `port` to stop and waits for its port to free.
///
/// # Errors
///
/// Returns the exit status to report: the daemon refused the credential, could not be asked, or did not stop in time.
pub async fn stop(port: u16, credential: &str, force: bool) -> Result<StopOutcome, ExitStatus> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    if !listening(&address) {
        return Ok(StopOutcome::NotRunning);
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| ExitStatus::Internal)?;
    let response = client
        .post(format!(
            "http://127.0.0.1:{port}/api/v1/shutdown{}",
            if force { "?force=true" } else { "" }
        ))
        .bearer_auth(credential)
        .send()
        .await
        .map_err(|_| {
            eprintln!("jarvis: the daemon could not be asked to stop");
            ExitStatus::Unavailable
        })?;
    if response.status().as_u16() == 409 {
        // Work is in flight and a stop would lose it: the daemon says how much, and is left running.
        let message = response
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|body| {
                body.get("message")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "tasks are still working".to_owned());
        return Ok(StopOutcome::Busy(message));
    }
    if !response.status().is_success() {
        eprintln!(
            "jarvis: the daemon refused the stop request (status {}); is this the profile it is running?",
            response.status().as_u16()
        );
        return Err(ExitStatus::Rejected);
    }
    let deadline = Instant::now() + STOP_WAIT;
    while Instant::now() < deadline {
        if !listening(&address) {
            return Ok(StopOutcome::Stopped);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    eprintln!(
        "jarvis: the daemon was asked to stop but still holds port {port} after {} seconds",
        STOP_WAIT.as_secs()
    );
    Err(ExitStatus::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing listening is reported as such, not as a failure.
    #[tokio::test]
    async fn stopping_what_is_not_running_is_not_an_error() {
        // Port 1 is never a JARVIS daemon and is refused immediately.
        assert_eq!(stop(1, "unused", false).await, Ok(StopOutcome::NotRunning));
    }
}
