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
pub enum Stopped {
    /// Nothing was listening, so there was nothing to stop.
    NotRunning,
    /// The daemon stopped and its port is free.
    Stopped,
}

/// Asks the daemon on `port` to stop and waits for its port to free.
///
/// # Errors
///
/// Returns the exit status to report: the daemon refused the credential, could not be asked, or did not stop in time.
pub async fn stop(port: u16, credential: &str) -> Result<Stopped, ExitStatus> {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    if !listening(&address) {
        return Ok(Stopped::NotRunning);
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| ExitStatus::Internal)?;
    let response = client
        .post(format!("http://127.0.0.1:{port}/api/v1/shutdown"))
        .bearer_auth(credential)
        .send()
        .await
        .map_err(|_| {
            eprintln!("jarvis: the daemon could not be asked to stop");
            ExitStatus::Unavailable
        })?;
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
            return Ok(Stopped::Stopped);
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
        assert_eq!(stop(1, "unused").await, Ok(Stopped::NotRunning));
    }
}
