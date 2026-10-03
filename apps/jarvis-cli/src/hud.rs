//! `jarvis hud`: opens the heads-up display.
//!
//! The page is served by the daemon and holds no data; the credential it needs is passed in the URL **fragment**, which
//! a browser never sends to a server (see `apps/jarvisd/src/hud.rs`). The command prints the address **without** the
//! credential, so it is not left in a terminal's scrollback; `--no-open` prints it and opens nothing.

use std::process::{Command, Stdio};

use crate::output::ExitStatus;

/// The address a browser is opened at, carrying the credential in the fragment.
fn address(port: u16, credential: &str) -> String {
    format!("http://127.0.0.1:{port}/hud#token={credential}")
}

/// Opens the display, or says how to.
pub fn open(port: u16, credential: &str, no_open: bool) -> ExitStatus {
    println!("display: http://127.0.0.1:{port}/hud");
    if no_open {
        println!("(not opened; the page needs the credential, which only `jarvis hud` passes)");
        return ExitStatus::Ok;
    }
    let target = address(port, credential);
    #[cfg(windows)]
    let launched = Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", &target])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(target_os = "macos")]
    let launched = Command::new("open")
        .arg(&target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let launched = Command::new("xdg-open")
        .arg(&target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match launched {
        Ok(status) if status.success() => ExitStatus::Ok,
        _ => {
            eprintln!("jarvis: no browser could be opened; use `jarvis watch` for a terminal view");
            ExitStatus::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credential_travels_in_the_fragment_and_never_in_the_query() {
        let target = address(8765, "abc123");
        assert_eq!(target, "http://127.0.0.1:8765/hud#token=abc123");
        assert!(!target.contains('?'));
    }
}
