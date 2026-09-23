//! Granted workspace roots, and the confinement guarantee a filesystem adapter is built on.
//!
//! `docs/architecture/tools-and-connectors.md` states the requirement for the filesystem adapter:
//! "explicit granted roots, handle-based resolution where possible, canonicalization plus
//! race-resistant open semantics, symlink/junction policy, byte/file-count bounds." The threat table
//! in `docs/architecture/security.md` repeats it as "Filesystem escape | granted roots;
//! handle-based/race-resistant access; symlink/junction policy; path normalization; size/count
//! limits."
//!
//! # Why confinement is a handle, not a validated path
//!
//! The obvious implementation is to join the caller's path onto a root, canonicalize the result, and
//! check the canonical path still begins with the root. It is **wrong**, and the requirement says why:
//! `canonicalize` resolves the path and then the open happens, and a component swapped for a symlink
//! between the two escapes the root. That is the "race" the requirement names, and no amount of
//! careful ordering removes it — the window is between two syscalls.
//!
//! So this module does not validate paths. It holds a [`cap_std::fs::Dir`] opened once on each granted
//! root, and every read resolves through **that handle**. On unix `cap-std` resolves beneath the
//! directory descriptor (using `openat2` with `RESOLVE_BENEATH` where the kernel supports it); on
//! Windows it resolves beneath the directory handle. A path that leaves the root cannot be expressed,
//! because each component is resolved relative to the handle rather than to the process's ambient
//! filesystem view.
//!
//! Two consequences worth stating, because both are the opposite of what a validation scheme gives:
//!
//! - **A symlink inside a root that points outside it is not "detected and rejected"** — it is
//!   followed, and the escape fails at the kernel boundary. The difference matters because detection
//!   is a check that can be bypassed, while the boundary is not.
//! - **An absolute path is not "rejected" either.** A leading `/` is an ordinary character to a
//!   handle-relative resolution, so `Dir::open("/etc/passwd")` cannot address the host's `/etc`. The
//!   tests assert this rather than trusting it.
//!
//! # Why this is a separate module from the adapter
//!
//! `WorkspaceRoots` is the security boundary and the adapter is the tool. Keeping them apart means the
//! boundary has its own tests that do not need a tool call, and an adapter author cannot accidentally
//! bypass it by reading a path with `std::fs` — there is no accessor on this type that hands out a
//! root as a plain [`std::path::Path`] for reading.

use std::io;
use std::path::{Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::Dir;

/// The reason a set of workspace roots was refused.
///
/// Every variant is a configuration fault rather than a runtime condition, because a root is granted
/// when a workspace is opened rather than chosen by a caller. That distinction is what lets the tool
/// adapter treat a failure here as [`crate::AdapterError::RefusedBeforeReaching`] rather than as an
/// ambiguous one.
#[derive(Clone, Debug, Eq, thiserror::Error, PartialEq)]
pub enum RootError {
    /// No root was granted.
    ///
    /// Refused rather than treated as "nothing is readable": an empty grant means the configuration
    /// is wrong, and a tool that silently reads nothing looks like a tool that works.
    #[error("at least one workspace root must be granted")]
    NoRoots,
    /// A granted root was not an absolute path.
    ///
    /// Relative to what is a question with no safe answer — the daemon's working directory is not a
    /// workspace, and a service manager may start it anywhere. So a relative root is refused rather
    /// than resolved against the process's current directory.
    #[error("workspace root {root} must be an absolute path")]
    RelativeRoot {
        /// The root as it was supplied.
        root: String,
    },
    /// A granted root could not be opened as a directory.
    #[error("workspace root {root} is not a readable directory: {reason}")]
    UnopenableRoot {
        /// The root as it was supplied.
        root: String,
        /// Why the root could not be opened, bounded.
        reason: String,
    },
    /// The same root was granted twice.
    ///
    /// Refused because a duplicate makes "which root does this file belong to?" ambiguous, and the
    /// first-matching-root answer would depend on the order roots were added — the kind of implicit
    /// ordering that turns a configuration into a behaviour.
    #[error("workspace root {root} was granted more than once")]
    DuplicateRoot {
        /// The duplicated root as it was supplied.
        root: String,
    },
}

/// One granted root: the path it was granted as, and the handle every read resolves through.
struct GrantedRoot {
    /// The path as supplied, for diagnostics and for naming a file's owning root.
    label: PathBuf,
    /// The confinement handle. Reads resolve beneath this and cannot leave it.
    handle: Dir,
}

/// The roots a workspace granted, held as open handles.
///
/// Not `Clone`, deliberately: the handle set is the boundary, and a copy would be a second place the
/// boundary is configured. Pass a reference.
pub struct WorkspaceRoots {
    roots: Vec<GrantedRoot>,
}

impl WorkspaceRoots {
    /// Opens every granted root as a confinement handle.
    ///
    /// # Errors
    ///
    /// Returns [`RootError::NoRoots`] for an empty grant, [`RootError::RelativeRoot`] for a
    /// non-absolute root, [`RootError::DuplicateRoot`] for a repeated root, and
    /// [`RootError::UnopenableRoot`] when a root is missing or is not a directory.
    ///
    /// A missing root is an error rather than being skipped: skipping it would mean a workspace whose
    /// disk was not mounted reads as a workspace with fewer files, which is indistinguishable from a
    /// tool that found nothing.
    pub fn new<I, P>(roots: I) -> Result<Self, RootError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut granted: Vec<GrantedRoot> = Vec::new();
        for root in roots {
            let root = root.as_ref();
            let label = root.to_path_buf();
            let display = label.display().to_string();
            if !root.is_absolute() {
                return Err(RootError::RelativeRoot { root: display });
            }
            if granted.iter().any(|existing| existing.label == label) {
                return Err(RootError::DuplicateRoot { root: display });
            }
            let handle = Dir::open_ambient_dir(root, ambient_authority()).map_err(|error| {
                RootError::UnopenableRoot {
                    root: display.clone(),
                    // `cap-std` reports `io::Error` directly, so no conversion is involved.
                    reason: error.to_string(),
                }
            })?;
            granted.push(GrantedRoot { label, handle });
        }
        if granted.is_empty() {
            return Err(RootError::NoRoots);
        }
        Ok(Self { roots: granted })
    }

    /// Returns how many roots were granted.
    #[must_use]
    pub fn len(&self) -> usize {
        self.roots.len()
    }

    /// Returns whether no roots were granted.
    ///
    /// Always `false` for a value built by [`Self::new`], because an empty grant is refused. It exists
    /// because clippy requires `is_empty` beside `len`, and it reports the true state rather than
    /// asserting an invariant the constructor already enforces.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Returns the granted roots, for diagnostics.
    #[must_use]
    pub fn labels(&self) -> Vec<&Path> {
        self.roots.iter().map(|root| root.label.as_path()).collect()
    }

    /// Resolves a caller-supplied relative path against the first root that can open it.
    ///
    /// The path is **not** pre-validated. Each root's handle is asked in grant order, and the first
    /// that can resolve the whole path beneath itself wins. A path that no root can resolve is
    /// [`RootError::UnopenableRoot`]-shaped only if the root itself is broken; otherwise the caller
    /// sees the underlying "not found" or "the resolution left the root" as a
    /// [`crate::AdapterError::RefusedBeforeReaching`] from the adapter.
    ///
    /// Returns the opened file and the label of the root that resolved it, so a result can say which
    /// grant answered without exposing the handle.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] when no root could open the path. On unix and Windows
    /// alike, an attempt to escape the root surfaces here as an error rather than as an open file —
    /// that is the guarantee this module exists to provide, and the tests prove it by running.
    pub fn open_file(&self, relative: &Path) -> Result<(cap_std::fs::File, &Path), io::Error> {
        let mut last_error: Option<io::Error> = None;
        for root in &self.roots {
            match root.handle.open(relative) {
                Ok(file) => return Ok((file, root.label.as_path())),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error
            .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no workspace root")))
    }

    /// Reads a directory's entries beneath the first root that can resolve it.
    ///
    /// Returns the entries sorted by name and the label of the resolving root. The entries are
    /// resolved through the root's handle, so a subdirectory that is a symlink out of the root cannot
    /// be listed through this call — opening the subdirectory would fail at the boundary.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] when no root could open the path.
    pub fn read_directory(&self, relative: &Path) -> Result<(Vec<String>, &Path), io::Error> {
        let mut last_error: Option<io::Error> = None;
        for root in &self.roots {
            match root.handle.read_dir(relative) {
                Ok(entries) => {
                    let mut names: Vec<String> = Vec::new();
                    for entry in entries {
                        // `cap-std` yields `io::Error` already, so there is nothing to convert.
                        let entry = entry?;
                        // A name that is not valid UTF-8 is reported, not skipped: skipping it would
                        // make a directory's listing differ from its contents with nothing saying so.
                        names.push(
                            entry
                                .file_name()
                                .to_str()
                                .map_or_else(|| String::from("<non-utf8 name>"), str::to_owned),
                        );
                    }
                    names.sort_unstable();
                    return Ok((names, root.label.as_path()));
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error
            .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no workspace root")))
    }
}

impl std::fmt::Debug for WorkspaceRoots {
    /// Lists the granted roots and nothing else.
    ///
    /// A directory handle has no useful debug form, and printing one would be the sort of internals
    /// leak that a `Debug` implementation on a security boundary should not offer.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceRoots")
            .field("roots", &self.labels())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A temporary directory removed when the test ends, pass or fail.
    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "jarvis-workspace-roots-{}",
                jarvis_core::scratch_tag()
            ));
            fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create root: {error}"));
            // The path is canonicalized because `std::env::temp_dir()` on Windows can return an
            // 8.3 short form (`C:\Users\RUNNER~1\...`) whose text differs from the long form a
            // handle reports. Comparing a short-form root against a long-form resolution is a
            // fixture bug that looks like a confinement failure.
            let path = path
                .canonicalize()
                .unwrap_or_else(|error| panic!("canonicalize root: {error}"));
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// Creates a file inside the directory, making parents, and returns its path.
        fn write(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
            }
            fs::write(&path, contents)
                .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
            path
        }

        fn make_dir(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(&path)
                .unwrap_or_else(|error| panic!("create dir {}: {error}", path.display()));
            path
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Creates a directory link at `link` pointing at `target`.
    ///
    /// **Platform-specific on purpose.** On unix this is a symlink. On Windows it is a **junction**,
    /// not a symlink, and the reason is the point of the test: creating a Windows symlink requires
    /// `SeCreateSymbolicLinkPrivilege` or Developer Mode, so an unprivileged user usually cannot make
    /// one — but **any** user can create a junction. The junction is therefore the escape vector that
    /// actually matters on Windows, and testing a symlink there would test a primitive the attacker
    /// often cannot use.
    ///
    /// A failure to create the link panics rather than skipping. A skipped link test passes while
    /// proving nothing, which is the failure mode this whole module exists to avoid.
    fn link_directory(target: &Path, link: &Path) {
        #[cfg(windows)]
        {
            let output = std::process::Command::new("cmd")
                .arg("/C")
                .arg("mklink")
                .arg("/J")
                .arg(link)
                .arg(target)
                .output()
                .unwrap_or_else(|error| panic!("run mklink: {error}"));
            assert!(
                output.status.success(),
                "a junction must be creatable by any user, so a failure here is a broken fixture: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link)
                .unwrap_or_else(|error| panic!("create symlink: {error}"));
        }
    }

    fn roots(paths: &[&Path]) -> WorkspaceRoots {
        WorkspaceRoots::new(paths.iter().copied())
            .unwrap_or_else(|error| panic!("roots must be accepted: {error}"))
    }

    /// **A path that climbs out of the root cannot be opened.**
    ///
    /// The sibling directory holds a readable file with an unmistakable name, so a failure here cannot
    /// be "the file happened not to exist".
    #[test]
    fn a_path_that_climbs_out_of_the_root_is_refused() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        outer.write("outside.txt", "outside the grant");
        outer.write("root/inside.txt", "inside the grant");

        let granted = roots(&[root.as_path()]);

        // The control: the same file opened by its real path IS reachable, so the root is working.
        let (mut file, _) = granted
            .open_file(Path::new("inside.txt"))
            .unwrap_or_else(|error| panic!("the positive control must open: {error}"));
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut file, &mut contents)
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(contents, "inside the grant");

        for escape in [
            "../outside.txt",
            "../../outside.txt",
            "../root/../outside.txt",
        ] {
            assert!(
                granted.open_file(Path::new(escape)).is_err(),
                "{escape} must not resolve out of the granted root"
            );
        }
    }

    /// **An absolute path cannot address the host filesystem through a root handle.**
    ///
    /// The path is resolved relative to the directory handle, so a leading separator is an ordinary
    /// character rather than the start of an absolute path. Both a unix-style and a Windows-style
    /// absolute path are tried, because the platform that runs the test is not the platform whose
    /// spelling an attacker would necessarily use.
    #[test]
    fn an_absolute_path_cannot_address_the_host_filesystem() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");
        let granted = roots(&[root.as_path()]);

        for absolute in [
            "/etc/passwd",
            "/etc/hosts",
            r"C:\Windows\win.ini",
            r"C:/Windows/win.ini",
            r"\\?\C:\Windows\win.ini",
        ] {
            assert!(
                granted.open_file(Path::new(absolute)).is_err(),
                "{absolute} must not resolve as an absolute host path"
            );
        }
    }

    /// **A directory link that points outside the root cannot be traversed.**
    ///
    /// This is the "symlink/junction policy" requirement, and it is the case a
    /// canonicalize-then-prefix-check implementation gets wrong: the check sees a path inside the root
    /// (the link is inside), the open follows the link, and the read lands outside.
    ///
    /// Here the link already exists before the call — that is the post-race state, so this test covers
    /// the outcome of a swap without needing to win a timing race. The separate property, that the
    /// window cannot be re-opened between a check and an open, follows from there being no check to
    /// race: the resolution happens once, beneath the handle.
    #[test]
    fn a_directory_link_out_of_the_root_cannot_be_traversed() {
        let outer = TestDirectory::new();
        let root = outer.make_dir("root");
        outer.write("outside/secret.txt", "outside the grant");

        // `root/escape` is INSIDE the granted root; it points at `outer/outside`, which is not.
        link_directory(&outer.path().join("outside"), &root.join("escape"));

        // **The control that makes the assertions below mean something.** The same path, read with
        // ambient authority (plain `std::fs`), DOES reach the file behind the link. Without this, a
        // refusal below could be a mistyped fixture or a link that was never created, and the test
        // would pass while proving nothing about confinement.
        let ambient =
            fs::read_to_string(root.join("escape").join("secret.txt")).unwrap_or_else(|error| {
                panic!("the escape must be real for ambient authority: {error}")
            });
        assert_eq!(ambient, "outside the grant");

        let granted = roots(&[root.as_path()]);

        // The link itself is visible — that is not the defect being tested.
        assert!(
            granted.open_file(Path::new("escape")).is_err()
                || granted.read_directory(Path::new("escape")).is_err(),
            "a link out of the root must not resolve through the root handle"
        );
        assert!(
            granted.open_file(Path::new("escape/secret.txt")).is_err(),
            "a file behind a link out of the root must not be readable"
        );
        assert!(
            granted.read_directory(Path::new("escape")).is_err(),
            "a link out of the root must not be listable"
        );
    }

    /// A file inside the root is read, and the root that resolved it is named.
    ///
    /// The label matters for an audit record: "which grant answered this call" is a question a
    /// multi-root workspace has to be able to answer.
    #[test]
    fn a_file_inside_a_root_is_read_and_its_root_is_named() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");
        directory.write("root/notes/todo.txt", "buy milk");

        let granted = roots(&[root.as_path()]);
        let (mut file, label) = granted
            .open_file(Path::new("notes/todo.txt"))
            .unwrap_or_else(|error| panic!("read: {error}"));

        let mut contents = String::new();
        std::io::Read::read_to_string(&mut file, &mut contents)
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(contents, "buy milk");
        assert_eq!(label, root.as_path());
    }

    /// A directory's entries are listed sorted, and a missing directory is an error.
    #[test]
    fn a_directory_is_listed_sorted() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");
        for name in ["charlie.txt", "alpha.txt", "bravo.txt"] {
            directory.write(&format!("root/{name}"), "x");
        }

        let granted = roots(&[root.as_path()]);
        let (names, label) = granted
            .read_directory(Path::new("."))
            .unwrap_or_else(|error| panic!("list: {error}"));
        assert_eq!(names, vec!["alpha.txt", "bravo.txt", "charlie.txt"]);
        assert_eq!(label, root.as_path());

        assert!(granted.read_directory(Path::new("nope")).is_err());
    }

    /// **Several roots are searched in grant order, and only the resolving one is reported.**
    #[test]
    fn the_first_root_that_can_resolve_a_path_answers_it() {
        let directory = TestDirectory::new();
        let first = directory.make_dir("first");
        let second = directory.make_dir("second");
        directory.write("first/shared.txt", "from the first root");
        directory.write("second/shared.txt", "from the second root");
        directory.write("second/only-second.txt", "only here");

        let granted = roots(&[first.as_path(), second.as_path()]);

        let (mut file, label) = granted
            .open_file(Path::new("shared.txt"))
            .unwrap_or_else(|error| panic!("read: {error}"));
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut file, &mut contents)
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(contents, "from the first root");
        assert_eq!(label, first.as_path(), "grant order decides, not proximity");

        // A path only the second root holds still resolves, so the search is not just the first root.
        let (mut file, label) = granted
            .open_file(Path::new("only-second.txt"))
            .unwrap_or_else(|error| panic!("read: {error}"));
        let mut contents = String::new();
        std::io::Read::read_to_string(&mut file, &mut contents)
            .unwrap_or_else(|error| panic!("read: {error}"));
        assert_eq!(contents, "only here");
        assert_eq!(label, second.as_path());
    }

    /// A grant that cannot be honoured is refused rather than quietly narrowed.
    ///
    /// Each case is a configuration fault, so each must be an error. The alternative — treating an
    /// unusable root as "no files there" — produces a workspace that reads as empty, which is
    /// indistinguishable from a working tool that found nothing.
    #[test]
    fn an_unusable_grant_is_refused() {
        let directory = TestDirectory::new();
        let root = directory.make_dir("root");

        assert_eq!(
            WorkspaceRoots::new(std::iter::empty::<&Path>()).err(),
            Some(RootError::NoRoots)
        );

        let file = directory.write("root/plain.txt", "x");
        assert!(
            matches!(
                WorkspaceRoots::new([file.as_path()]).err(),
                Some(RootError::UnopenableRoot { .. })
            ),
            "a file is not a root"
        );

        let missing = root.join("does-not-exist");
        assert!(
            matches!(
                WorkspaceRoots::new([missing.as_path()]).err(),
                Some(RootError::UnopenableRoot { .. })
            ),
            "a missing root must be refused, not skipped"
        );

        assert!(
            matches!(
                WorkspaceRoots::new([Path::new("relative/path")]).err(),
                Some(RootError::RelativeRoot { .. })
            ),
            "a relative root has no safe meaning for a daemon"
        );

        assert!(
            matches!(
                WorkspaceRoots::new([root.as_path(), root.as_path()]).err(),
                Some(RootError::DuplicateRoot { .. })
            ),
            "a duplicate root would make the resolving root order-dependent"
        );

        // And the control: a single usable root IS accepted, so the refusals above are about the
        // specific faults and not about the constructor refusing everything.
        let granted = roots(&[root.as_path()]);
        assert_eq!(granted.len(), 1);
        assert!(!granted.is_empty());
    }
}
