//! `jarvis hud`: opens the heads-up display.
//!
//! The page is served by the daemon and holds no data; the credential it needs is passed in the URL **fragment**, which
//! a browser never sends to a server (see `apps/jarvisd/src/hud.rs`). The command prints the address **without** the
//! credential, so it is not left in a terminal's scrollback; `--no-open` prints it and opens nothing.

use std::process::{Command, Stdio};

use crate::output::ExitStatus;

/// The address a browser is opened at, carrying the credential in the fragment.
fn address(port: u16, credential: &str) -> String {
    format!("http://127.0.0.1:{port}/#token={credential}")
}

/// Whether this machine has no display to open a browser on (a Linux server over SSH).
const fn is_headless(display_set: bool, wayland_set: bool) -> bool {
    cfg!(all(unix, not(target_os = "macos"))) && !display_set && !wayland_set
}

/// What to do on a machine with no display: reach the console from your own computer through an SSH tunnel.
fn tunnel_hint(port: u16) -> String {
    format!(
        "this machine has no display, so nothing was opened. From your own computer:\n  \
         ssh -L {port}:127.0.0.1:{port} USER@THIS-HOST\nthen open the address that `jarvis hud --print-url` prints here."
    )
}

/// Opens the display, or says how to.
///
/// `print_url` prints the full address, **including the credential**, instead of opening anything: it is how a console on a
/// server is opened from another computer. It is only printed when asked for, because it would otherwise sit in a log.
pub fn open(port: u16, credential: &str, no_open: bool, print_url: bool) -> ExitStatus {
    println!("display: http://127.0.0.1:{port}/");
    if print_url {
        println!("open: {}", address(port, credential));
        println!(
            "(that address contains your credential; do not share it or paste it anywhere public)"
        );
        return ExitStatus::Ok;
    }
    if is_headless(
        std::env::var_os("DISPLAY").is_some(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
    ) {
        println!("{}", tunnel_hint(port));
        return ExitStatus::Ok;
    }
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
    fn a_machine_with_no_display_is_told_how_to_tunnel_in() {
        // Only a Linux-like machine with neither display variable counts as headless; macOS and Windows always have one.
        assert!(!is_headless(true, false));
        assert!(!is_headless(false, true));
        assert_eq!(
            is_headless(false, false),
            cfg!(all(unix, not(target_os = "macos")))
        );
        let hint = tunnel_hint(8765);
        assert!(hint.contains("ssh -L 8765:127.0.0.1:8765"));
        assert!(
            !hint.contains("token"),
            "the hint must not carry the credential"
        );
    }

    #[test]
    fn the_credential_travels_in_the_fragment_and_never_in_the_query() {
        let target = address(8765, "abc123");
        assert_eq!(target, "http://127.0.0.1:8765/#token=abc123");
        assert!(!target.contains('?'));
    }
}
