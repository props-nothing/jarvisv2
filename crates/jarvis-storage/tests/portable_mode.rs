//! Acceptance evidence for portable mode (`FR-INSTALL-003`, `P1-011`).
//!
//! `docs/operations/install-and-release.md` defines portable mode as: extract and
//! run with an **explicit** profile/data directory, and make no PATH, service,
//! registry, `LaunchAgent`, or systemd changes. `docs/research/integrations/os-application-directories.md`
//! adds the constraint that a relative root is never silently promoted to the
//! current directory.
//!
//! The tests below assert containment rather than absence: it is not enough that
//! portable mode does not write to the OS locations, it must write **everything**
//! inside the supplied root.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use jarvis_core::{LocalEndpoint, is_valid_profile};
use jarvis_storage::{AppPaths, PathMode, RuntimePathSource, portable_layout};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "jarvis-portable-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap_or_else(|error| panic!("create temp dir: {error}"));
        Self(path)
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_portable_root_contains_every_managed_directory() {
    let root = TempDirectory::new("containment");
    let paths = portable_layout(&root.0).unwrap_or_else(|| panic!("absolute root must resolve"));

    for (label, path) in [
        ("config", paths.config()),
        ("data", paths.data()),
        ("cache", paths.cache()),
        ("state", paths.state()),
        ("runtime", paths.runtime()),
        ("logs", paths.logs()),
    ] {
        assert!(
            path.starts_with(&root.0),
            "{label} directory {path:?} escaped the portable root"
        );
    }
    assert_eq!(paths.mode(), PathMode::Portable);
    assert_eq!(paths.runtime_source(), RuntimePathSource::PortableRoot);
}

#[test]
fn a_relative_root_is_refused_rather_than_resolved_against_the_cwd() {
    // The silent-cwd fallback is the failure this guards: a relative root would
    // scatter state wherever the process happened to start.
    assert!(portable_layout(Path::new("jarvis-data")).is_none());
    assert!(portable_layout(Path::new("./jarvis-data")).is_none());
    assert!(AppPaths::from_root(Path::new("relative")).is_err());
}

#[test]
fn portability_is_a_property_of_the_root_not_the_working_directory() {
    // Two roots must not share any managed directory, or one installation could
    // read another's state simply because both were launched from one directory.
    let first = TempDirectory::new("isolated-a");
    let second = TempDirectory::new("isolated-b");

    let a = portable_layout(&first.0).unwrap_or_else(|| panic!("root a"));
    let b = portable_layout(&second.0).unwrap_or_else(|| panic!("root b"));

    assert_ne!(a.data(), b.data());
    assert_ne!(a.config(), b.config());
    assert_ne!(a.runtime(), b.runtime());
    assert!(!a.data().starts_with(&second.0));
    assert!(!b.data().starts_with(&first.0));
}

#[test]
fn separate_roots_never_share_a_local_endpoint() {
    // A shared endpoint would make two portable installations contend for one
    // socket or pipe, so the endpoint is derived from the runtime directory.
    let first = TempDirectory::new("endpoint-a");
    let second = TempDirectory::new("endpoint-b");
    let a = portable_layout(&first.0).unwrap_or_else(|| panic!("root a"));
    let b = portable_layout(&second.0).unwrap_or_else(|| panic!("root b"));

    let endpoint_a = LocalEndpoint::scoped(a.runtime(), "default")
        .unwrap_or_else(|error| panic!("endpoint a: {error}"));
    let endpoint_b = LocalEndpoint::scoped(b.runtime(), "default")
        .unwrap_or_else(|error| panic!("endpoint b: {error}"));
    assert_ne!(endpoint_a, endpoint_b);

    // The same root must derive the same endpoint, or a restarting client would
    // not find the daemon it just started.
    let repeated = LocalEndpoint::scoped(a.runtime(), "default")
        .unwrap_or_else(|error| panic!("endpoint a again: {error}"));
    assert_eq!(endpoint_a, repeated);

    // And the profile name is still validated, so portability did not weaken it.
    assert!(!is_valid_profile("bad/name"));
}

#[test]
fn preparing_a_portable_root_creates_only_directories_inside_it() {
    let root = TempDirectory::new("prepare");
    let paths = portable_layout(&root.0).unwrap_or_else(|| panic!("absolute root must resolve"));
    paths
        .prepare()
        .unwrap_or_else(|error| panic!("prepare portable root: {error}"));

    let mut entries = Vec::new();
    for entry in fs::read_dir(&root.0).unwrap_or_else(|error| panic!("read root: {error}")) {
        let entry = entry.unwrap_or_else(|error| panic!("entry: {error}"));
        assert!(
            entry
                .file_type()
                .unwrap_or_else(|error| panic!("file type: {error}"))
                .is_dir(),
            "portable preparation created a file, not a directory"
        );
        entries.push(entry.file_name().to_string_lossy().into_owned());
    }
    entries.sort();
    assert_eq!(
        entries,
        vec!["cache", "config", "data", "logs", "runtime", "state"]
    );
}

#[test]
fn the_root_itself_is_not_reported_as_a_managed_directory() {
    // Preparation must not claim the operator's own directory as JARVIS-owned.
    let root = TempDirectory::new("ownership");
    let paths = portable_layout(&root.0).unwrap_or_else(|| panic!("absolute root must resolve"));

    assert!(!paths.config().starts_with(paths.data()));
    assert!(!paths.logs().starts_with(paths.config()));
    // Every managed path is a strict descendant of the root.
    assert_eq!(paths.data().parent(), Some(root.0.as_path()));
}
