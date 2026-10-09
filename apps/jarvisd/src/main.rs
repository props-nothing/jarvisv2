//! The `jarvisd` binary: the daemon, started by name. `jarvis daemon` runs the same code.

use std::process::ExitCode;

fn main() -> ExitCode {
    jarvisd::run_blocking(std::env::args().skip(1).collect())
}
