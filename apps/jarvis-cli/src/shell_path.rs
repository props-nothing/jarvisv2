//! `jarvis path install|uninstall|status`: make `jarvis` work from any terminal (`P9-026`).
//!
//! A built or unpacked JARVIS is two programs in a folder, and nothing puts that folder on the search path, so the
//! suggestions JARVIS prints (`jarvis hud`, `jarvis chat`) fail with "command not found" until it is. On Windows the folder
//! is added to the user's `Path` (a new terminal sees it). On Linux and macOS both programs are linked into `~/.local/bin`,
//! which most shells already search. Nothing needs administrator rights, and `uninstall` removes only what `install` made.

use std::path::{Path, PathBuf};

use crate::output::ExitStatus;

/// Whether `directory` is one of the entries of a search-path value.
///
/// Compared without regard to case or a trailing separator on Windows, where both are insignificant.
#[must_use]
pub fn path_contains(search_path: &std::ffi::OsStr, directory: &Path) -> bool {
    let wanted = normalize(directory);
    std::env::split_paths(search_path).any(|entry| normalize(&entry) == wanted)
}

fn normalize(path: &Path) -> String {
    let text = path.to_string_lossy();
    let trimmed = text.trim_end_matches(['/', '\\']);
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed.to_owned()
    }
}

/// The PowerShell that adds (or removes) `directory` in the user's `Path`, and says which it did.
///
/// A single-quoted PowerShell string is literal except for the quote itself, which is doubled, so the directory cannot
/// end the string and run anything. The script reads the *user* value (never the machine one) so nothing else is rewritten.
#[cfg(any(windows, test))]
#[must_use]
pub fn windows_script(directory: &str, add: bool) -> String {
    let quoted = directory.replace('\'', "''");
    let head = format!(
        "$d='{quoted}'; $u=[Environment]::GetEnvironmentVariable('Path','User'); \
         $parts=@(); if($u){{$parts=@($u -split ';' | Where-Object {{ $_ -ne '' }})}}; \
         $same={{ param($x) $x.TrimEnd('\\') -ieq $d.TrimEnd('\\') }}; "
    );
    let tail = if add {
        "if(-not ($parts | Where-Object { & $same $_ })){ $parts += $d; \
         [Environment]::SetEnvironmentVariable('Path',($parts -join ';'),'User'); 'added' } else { 'already' }"
    } else {
        "$keep=@($parts | Where-Object { -not (& $same $_) }); \
         if($keep.Count -ne $parts.Count){ [Environment]::SetEnvironmentVariable('Path',($keep -join ';'),'User'); 'removed' } else { 'absent' }"
    };
    format!("{head}{tail}")
}

/// The links `install` makes on Linux and macOS: `jarvis`, from `bin_dir` to the real file, and `jarvisd` too when an older
/// release left one beside it (the daemon is now `jarvis daemon`, `ADR-0144`).
#[cfg(any(not(windows), test))]
#[must_use]
pub fn unix_links(program_dir: &Path, bin_dir: &Path) -> Vec<(PathBuf, PathBuf)> {
    ["jarvis", "jarvisd"]
        .iter()
        .filter(|name| **name == "jarvis" || program_dir.join(name).exists())
        .map(|name| (bin_dir.join(name), program_dir.join(name)))
        .collect()
}

fn program_dir() -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("this program's location is unknown: {error}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "this program has no folder".to_owned())?
        .to_path_buf();
    // Windows can report the extended-length form, which a search path does not use.
    let text = dir.to_string_lossy();
    Ok(text
        .strip_prefix(r"\\?\")
        .map_or(dir.clone(), PathBuf::from))
}

fn on_path_now(dir: &Path) -> bool {
    std::env::var_os("PATH").is_some_and(|value| path_contains(&value, dir))
}

/// A one-line tip for the end of `jarvis start` when `jarvis` would not be found from another terminal.
#[must_use]
pub fn hint() -> Option<String> {
    let dir = program_dir().ok()?;
    if on_path_now(&dir) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    Some(format!(
        "tip: `jarvis` is not on your PATH, so `jarvis hud` will not work from another terminal. Run: \"{}\" path install",
        exe.display()
    ))
}

/// `jarvis path <install|uninstall|status>`
pub fn run(arguments: &[String]) -> ExitStatus {
    let action = arguments.get(1).map(String::as_str);
    let dir = match program_dir() {
        Ok(dir) => dir,
        Err(message) => {
            eprintln!("jarvis: {message}");
            return ExitStatus::Internal;
        }
    };
    match action {
        Some("status") | None => {
            if on_path_now(&dir) {
                println!(
                    "{} is on your PATH: `jarvis` works from any terminal.",
                    dir.display()
                );
            } else {
                println!(
                    "{} is not on your PATH. Run `jarvis path install` (from this folder) to fix that.",
                    dir.display()
                );
            }
            ExitStatus::Ok
        }
        Some("install") => install(&dir),
        Some("uninstall") => uninstall(&dir),
        Some(other) => {
            eprintln!("jarvis: path {other:?} is not a command; use install, uninstall or status");
            ExitStatus::Usage
        }
    }
}

#[cfg(windows)]
fn change_user_path(dir: &Path, add: bool) -> ExitStatus {
    let script = windows_script(&dir.to_string_lossy(), add);
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(std::process::Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let said = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            match (add, said.as_str()) {
                (true, "added") => println!(
                    "added {} to your user PATH. Open a new terminal, then `jarvis` works anywhere.",
                    dir.display()
                ),
                (true, _) => println!("{} was already on your user PATH.", dir.display()),
                (false, "removed") => println!("removed {} from your user PATH.", dir.display()),
                (false, _) => println!("{} was not on your user PATH.", dir.display()),
            }
            ExitStatus::Ok
        }
        Ok(out) => {
            eprintln!(
                "jarvis: the PATH could not be changed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            ExitStatus::Unavailable
        }
        Err(error) => {
            eprintln!("jarvis: powershell.exe could not be run: {error}");
            ExitStatus::Unavailable
        }
    }
}

#[cfg(windows)]
fn install(dir: &Path) -> ExitStatus {
    change_user_path(dir, true)
}

#[cfg(windows)]
fn uninstall(dir: &Path) -> ExitStatus {
    change_user_path(dir, false)
}

#[cfg(not(windows))]
fn bin_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "HOME is not set".to_owned())?;
    Ok(PathBuf::from(home).join(".local").join("bin"))
}

#[cfg(not(windows))]
fn install(dir: &Path) -> ExitStatus {
    let bin = match bin_dir() {
        Ok(bin) => bin,
        Err(message) => {
            eprintln!("jarvis: {message}");
            return ExitStatus::Internal;
        }
    };
    if let Err(error) = std::fs::create_dir_all(&bin) {
        eprintln!("jarvis: {} could not be created: {error}", bin.display());
        return ExitStatus::Internal;
    }
    for (link, target) in unix_links(dir, &bin) {
        if !target.exists() {
            eprintln!(
                "jarvis: {} is not there, so it was not linked",
                target.display()
            );
            continue;
        }
        if std::fs::read_link(&link).is_ok_and(|existing| existing == target) {
            println!("{} already points at {}", link.display(), target.display());
            continue;
        }
        // Something else is there: a different JARVIS, or a file that is not ours. It is left alone.
        if link.symlink_metadata().is_ok() {
            eprintln!(
                "jarvis: {} exists and is not a link to this JARVIS, so it was left alone",
                link.display()
            );
            continue;
        }
        if let Err(error) = std::os::unix::fs::symlink(&target, &link) {
            eprintln!("jarvis: {} could not be linked: {error}", link.display());
            return ExitStatus::Internal;
        }
        println!("linked {} -> {}", link.display(), target.display());
    }
    if on_path_now(&bin) {
        println!("`jarvis` now works from any terminal.");
    } else {
        println!(
            "{} is not on your PATH yet. Add it to your shell profile: export PATH=\"$HOME/.local/bin:$PATH\"",
            bin.display()
        );
    }
    ExitStatus::Ok
}

#[cfg(not(windows))]
fn uninstall(dir: &Path) -> ExitStatus {
    let bin = match bin_dir() {
        Ok(bin) => bin,
        Err(message) => {
            eprintln!("jarvis: {message}");
            return ExitStatus::Internal;
        }
    };
    for (link, target) in unix_links(dir, &bin) {
        // Only a link that points at this JARVIS is removed.
        if std::fs::read_link(&link).is_ok_and(|existing| existing == target) {
            if let Err(error) = std::fs::remove_file(&link) {
                eprintln!("jarvis: {} could not be removed: {error}", link.display());
                return ExitStatus::Internal;
            }
            println!("removed {}", link.display());
        }
    }
    ExitStatus::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_directory_is_found_on_the_search_path_by_exact_entry() {
        let sep = if cfg!(windows) { ';' } else { ':' };
        let dir = if cfg!(windows) {
            r"C:\tools\jarvis"
        } else {
            "/opt/jarvis"
        };
        let other = if cfg!(windows) {
            r"C:\Windows"
        } else {
            "/usr/bin"
        };
        let value = format!("{other}{sep}{dir}");
        assert!(path_contains(std::ffi::OsStr::new(&value), Path::new(dir)));
        // A parent or a sibling is not the directory.
        let parent = Path::new(dir).parent().unwrap_or_else(|| Path::new("/"));
        assert!(!path_contains(
            std::ffi::OsStr::new(&value),
            &parent.join("other")
        ));
        assert!(!path_contains(
            std::ffi::OsStr::new(&format!("{dir}x")),
            Path::new(dir)
        ));
    }

    #[test]
    fn a_trailing_separator_does_not_hide_the_directory() {
        let dir = if cfg!(windows) {
            r"C:\tools\jarvis"
        } else {
            "/opt/jarvis"
        };
        let with = format!("{dir}{}", if cfg!(windows) { "\\" } else { "/" });
        assert!(path_contains(std::ffi::OsStr::new(&with), Path::new(dir)));
    }

    /// The script is run by PowerShell, so a quote in the directory must not be able to end the string and run anything.
    #[test]
    fn a_quote_in_the_directory_cannot_leave_the_powershell_string() {
        let script = windows_script("C:\\evil'; Remove-Item C:\\x; '", true);
        assert!(
            script.contains("$d='C:\\evil''; Remove-Item C:\\x; ''';"),
            "{script}"
        );
    }

    #[test]
    fn the_script_changes_only_the_user_path_and_is_idempotent_by_construction() {
        let add = windows_script("C:\\tools\\jarvis", true);
        assert!(add.contains("GetEnvironmentVariable('Path','User')"));
        assert!(add.contains("SetEnvironmentVariable('Path',"));
        assert!(add.contains("'User')"));
        assert!(!add.contains("'Machine'"));
        // Adding checks for the entry first, so running it twice adds it once.
        assert!(add.contains("-not ($parts | Where-Object"));
        let remove = windows_script("C:\\tools\\jarvis", false);
        assert!(remove.contains("'removed'") && remove.contains("'absent'"));
    }

    #[test]
    fn only_the_one_executable_is_linked_unless_an_old_daemon_binary_is_beside_it() {
        let links = unix_links(Path::new("/opt/jarvis"), Path::new("/home/me/.local/bin"));
        assert_eq!(
            links,
            [(
                PathBuf::from("/home/me/.local/bin/jarvis"),
                PathBuf::from("/opt/jarvis/jarvis")
            )]
        );
        let old = std::env::temp_dir().join(format!("jarvis-links-{}", std::process::id()));
        std::fs::create_dir_all(&old).unwrap_or_else(|error| panic!("create: {error}"));
        std::fs::write(old.join("jarvisd"), b"old")
            .unwrap_or_else(|error| panic!("write: {error}"));
        let with_old = unix_links(&old, Path::new("/home/me/.local/bin"));
        let _ = std::fs::remove_dir_all(&old);
        assert_eq!(with_old.len(), 2);
        assert_eq!(with_old[1].0, PathBuf::from("/home/me/.local/bin/jarvisd"));
    }
}
