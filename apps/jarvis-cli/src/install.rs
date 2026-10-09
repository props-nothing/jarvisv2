//! `jarvis install` and `jarvis uninstall`: put the single executable in its place, in one step.
//!
//! JARVIS is one file (`ADR-0144`), so installing is copying it somewhere permanent and telling the system where it is. This does
//! that for the current user only, with no administrator rights and nothing outside the user's own folders:
//!
//! 1. copy this program to a per-user folder (`%LOCALAPPDATA%\Programs\JARVIS` on Windows, `~/.local/share/jarvis` elsewhere);
//! 2. run the installed copy's `path install`, so `jarvis` works from any new terminal;
//! 3. with `--service`, run its `service install`, so the assistant starts at login.
//!
//! Configuration, keys, memory and history live in the profile directories and are never touched; `uninstall` removes the program,
//! the path entry and the service, and says that the profile was left.

use std::path::{Path, PathBuf};

use crate::output::ExitStatus;

/// The per-user folder the program is installed in, from the two environment values it depends on.
///
/// Pure, so the choice is testable on every platform. `None` when the value it needs is missing.
#[must_use]
pub fn install_dir(
    windows: bool,
    local_app_data: Option<&Path>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    if windows {
        local_app_data
            .filter(|path| path.is_absolute())
            .map(|path| path.join("Programs").join("JARVIS"))
    } else {
        home.filter(|path| path.is_absolute())
            .map(|path| path.join(".local").join("share").join("jarvis"))
    }
}

/// The installed program's file name on this platform.
#[must_use]
pub const fn program_name() -> &'static str {
    if cfg!(windows) {
        "jarvis.exe"
    } else {
        "jarvis"
    }
}

/// Copies `source` into `dir` as the installed program, replacing an older copy, and returns the installed path.
///
/// Written to a temporary name beside the target and renamed over it, so an interrupted copy never leaves a half-written program
/// where a working one was.
///
/// # Errors
///
/// Returns a message when the folder cannot be made, the copy fails, or the old program is in use (Windows refuses to replace a
/// running executable).
pub fn copy_into(source: &Path, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("{} could not be created: {error}", dir.display()))?;
    let target = dir.join(program_name());
    let staging = dir.join(format!("{}.new", program_name()));
    std::fs::copy(source, &staging)
        .map_err(|error| format!("the program could not be copied: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("the program could not be made executable: {error}"))?;
    }
    if let Err(error) = std::fs::rename(&staging, &target) {
        let _ = std::fs::remove_file(&staging);
        return Err(format!(
            "the installed program could not be replaced (is JARVIS running? run `jarvis stop` first): {error}"
        ));
    }
    Ok(target)
}

fn directory(arguments: &[String]) -> Result<PathBuf, String> {
    if let Some(index) = arguments.iter().position(|argument| argument == "--dir") {
        let value = arguments.get(index + 1).ok_or("--dir needs a folder")?;
        let path = PathBuf::from(value);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err("--dir must be an absolute folder".to_owned())
        };
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from);
    install_dir(cfg!(windows), local.as_deref(), home.as_deref()).ok_or_else(|| {
        "could not work out where to install; pass --dir with an absolute folder".to_owned()
    })
}

/// Runs the installed program with arguments, passing its output through, and reports whether it succeeded.
fn run_installed(program: &Path, arguments: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(arguments)
        .status()
        .is_ok_and(|status| status.success())
}

/// `jarvis install [--dir DIR] [--service] [--no-path]`.
pub fn install(arguments: &[String]) -> ExitStatus {
    let dir = match directory(arguments) {
        Ok(dir) => dir,
        Err(message) => {
            eprintln!("jarvis: {message}");
            return ExitStatus::Usage;
        }
    };
    let Ok(source) = std::env::current_exe() else {
        eprintln!("jarvis: could not locate this program to install it");
        return ExitStatus::Internal;
    };
    let already_there = source.parent().is_some_and(|parent| parent == dir);
    let installed = if already_there {
        println!(
            "{} is already installed in {}",
            program_name(),
            dir.display()
        );
        source
    } else {
        match copy_into(&source, &dir) {
            Ok(path) => {
                println!("installed {}", path.display());
                path
            }
            Err(message) => {
                eprintln!("jarvis: {message}");
                return ExitStatus::Unavailable;
            }
        }
    };
    let mut ok = true;
    if !arguments.iter().any(|argument| argument == "--no-path") {
        ok &= run_installed(&installed, &["path", "install"]);
    }
    if arguments.iter().any(|argument| argument == "--service") {
        ok &= run_installed(&installed, &["service", "install"]);
    }
    println!("next:  open a new terminal and run `jarvis`   (the first run sets itself up)");
    if ok {
        ExitStatus::Ok
    } else {
        ExitStatus::Unavailable
    }
}

/// `jarvis uninstall [--dir DIR]`: removes the program, its path entry and its login service. The profile is left.
pub fn uninstall(arguments: &[String]) -> ExitStatus {
    let dir = match directory(arguments) {
        Ok(dir) => dir,
        Err(message) => {
            eprintln!("jarvis: {message}");
            return ExitStatus::Usage;
        }
    };
    let installed = dir.join(program_name());
    if !installed.is_file() {
        eprintln!("jarvis: nothing is installed in {}", dir.display());
        return ExitStatus::Unavailable;
    }
    let _ = run_installed(&installed, &["stop"]);
    let _ = run_installed(&installed, &["service", "uninstall"]);
    let _ = run_installed(&installed, &["path", "uninstall"]);
    let running_here = std::env::current_exe().is_ok_and(|exe| exe == installed);
    if running_here {
        println!(
            "the path entry and the login service are removed; delete {} yourself (a program cannot delete itself while it runs)",
            installed.display()
        );
    } else if let Err(error) = std::fs::remove_file(&installed) {
        eprintln!(
            "jarvis: {} could not be removed: {error}",
            installed.display()
        );
        return ExitStatus::Unavailable;
    } else {
        let _ = std::fs::remove_dir(&dir);
        println!("removed {}", installed.display());
    }
    println!("your settings, keys, memory and history were left in place");
    ExitStatus::Ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_is_per_user_and_needs_an_absolute_base() {
        let abs = if cfg!(windows) {
            "C:/Users/me"
        } else {
            "/home/me"
        };
        let local = Path::new(abs).join("AppData").join("Local");
        assert_eq!(
            install_dir(true, Some(&local), None),
            Some(local.join("Programs").join("JARVIS"))
        );
        assert_eq!(
            install_dir(false, None, Some(Path::new(abs))),
            Some(Path::new(abs).join(".local").join("share").join("jarvis"))
        );
        // A relative or missing base is refused rather than installing into whatever directory the shell was in.
        assert_eq!(install_dir(true, Some(Path::new("relative")), None), None);
        assert_eq!(install_dir(false, None, None), None);
    }

    fn scratch() -> PathBuf {
        let tag = jarvis_core::scratch_tag();
        let path = std::env::temp_dir().join(format!("jin-{}", &tag[tag.len() - 12..]));
        std::fs::create_dir_all(&path).unwrap_or_else(|error| panic!("{error}"));
        path
    }

    /// **Installing copies the program, replaces an older copy, and leaves no half-written file behind.**
    #[test]
    fn a_copy_replaces_an_older_one_and_leaves_no_staging_file() {
        let root = scratch();
        let source = root.join("source-program");
        std::fs::write(&source, b"new build").unwrap_or_else(|error| panic!("{error}"));
        let dir = root.join("nested").join("JARVIS");

        let first = copy_into(&source, &dir).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(std::fs::read(&first).unwrap_or_default(), b"new build");

        std::fs::write(&source, b"newer build").unwrap_or_else(|error| panic!("{error}"));
        let second = copy_into(&source, &dir).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(first, second);
        assert_eq!(std::fs::read(&second).unwrap_or_default(), b"newer build");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("{error}"))
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(leftovers, [program_name()], "no .new file is left behind");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&second).map_or(0, |meta| meta.permissions().mode());
            assert_eq!(mode & 0o111, 0o111, "the installed program is executable");
        }
        jarvis_core::remove_scratch_dir(&root);
    }

    #[test]
    fn a_missing_source_is_an_error_not_an_empty_install() {
        let root = scratch();
        let result = copy_into(&root.join("does-not-exist"), &root.join("out"));
        assert!(result.is_err());
        assert!(!root.join("out").join(program_name()).exists());
        jarvis_core::remove_scratch_dir(&root);
    }

    #[test]
    fn a_relative_dir_is_refused() {
        assert!(directory(&["install".to_owned(), "--dir".to_owned(), "here".to_owned()]).is_err());
        assert!(directory(&["install".to_owned(), "--dir".to_owned()]).is_err());
    }
}
