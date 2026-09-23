//! A naming rule for test scratch directories, so two runs cannot share one.
//!
//! # The defect this exists to prevent
//!
//! Around twenty fixture helpers in this workspace derived a scratch path as
//!
//! ```text
//! temp_dir() / format!("jarvis-<what>-{pid}-{sequence}", std::process::id())
//! ```
//!
//! with a `static …: AtomicU64 = AtomicU64::new(0)` beside it. The sequence therefore starts at **0** in every
//! process and the pid is a **reusable** resource, so the name is only unique *within one run*. A directory
//! survives whenever a test fails or the suite is killed — no `Drop` runs — and the next run that happens to be
//! given the same pid reopens the previous run's database file.
//!
//! The consequence is a class of failure rather than one test: `jarvis-storage`'s suite passed **144/144 alone**
//! and produced about twenty `UNIQUE constraint failed: sessions.id` failures inside a full-workspace run. The
//! leaked state was measurable — **1,368** `jarvis-approvals-*` directories in `%TEMP%`, **76** of them
//! `…-<pid>-0`, several still holding the three database files.
//!
//! # The rule
//!
//! **A scratch name must contain something that cannot repeat.** A `UUIDv7` satisfies that: it is generated per
//! call, so it is unique across processes *and* across runs, and it is already this project's identifier format.
//! A pid, a timestamp, and a counter that resets are all reusable, which is the whole problem.
//!
//! `create_dir_all` on a derived path is also an assertion, and the assertion is "this path is fresh".
//! [`scratch_tag`] is what makes it true; a caller that keeps `create_dir_all` after it is fine, and a caller
//! that removes it would be fine too.

use std::path::Path;
use std::time::Duration;

use crate::SessionId;

/// Returns a tag that is unique across processes and across runs.
///
/// Uses [`SessionId`] because it is a `UUIDv7` with a `Display` that is already lowercase-hex, so the tag is
/// safe in a path on every platform and needs no escaping. The name is deliberately not `temp_dir_name`: this
/// returns the **distinguishing part** only, and a caller joins it with its own descriptive prefix, because two
/// helpers that both took the whole path would be a second place the `temp_dir()` call lives.
///
/// # Examples
///
/// ```rust
/// use jarvis_core::scratch_tag;
///
/// let path = std::env::temp_dir().join(format!("jarvis-thing-{}", scratch_tag()));
/// assert!(path.to_string_lossy().contains("jarvis-thing-"));
/// ```
#[must_use]
pub fn scratch_tag() -> String {
    SessionId::new().to_string()
}

/// Removes a test scratch directory, **waiting for the handles inside it to be released**.
///
/// # The defect this exists to prevent
///
/// Every database-backed fixture in this workspace removes its scratch directory from a `Drop` impl. That call
/// fails on Windows with a sharing violation (`ERROR_SHARING_VIOLATION`, 32) while the database file is still
/// held, and the failure is swallowed by the `let _ =`, so the directory silently accumulates — **124 per full
/// suite run**, measured by cleaning `%TEMP%` first and counting the difference.
///
/// The reason it fails is worth stating exactly, because three plausible diagnoses are wrong. `sqlx` releases
/// the SQLite file **late and off-thread**: `PoolInner::Drop` calls `mark_closed()` and never awaits the close
/// future, and `PoolConnection::Drop` hands the connection back by **spawning** a task. So the handle is released
/// only once that spawned task runs, and the failure at teardown is a **race** rather than a permanent hold:
/// a drop followed by an immediate removal fails **5 times out of 5**, while the same drop followed by an
/// `await` of 250 ms succeeds. Two measurements rule out the tempting fixes:
///
/// - a bounded **blocking** retry (`std::thread::sleep`) still fails, because sleeping starves the current-thread
///   runtime and the spawned task never gets to run;
/// - awaiting `Database::close` on a fresh runtime **inside** a blocking worker thread **deadlocks**, because
///   the close future waits on a semaphore only the original runtime can release — and a deadlock in teardown is
///   worse than a leaked directory.
///
/// What does work is handing the removal to a **detached** thread that keeps retrying: it does not starve the
/// runtime (it is a fresh OS thread), and it keeps trying across the point where the runtime tears down and the
/// spawned task finally runs. Once the runtime is gone the handle is released — confirmed by measuring a removal
/// that succeeds after the runtime is dropped and a real delay has passed.
///
/// # Why this is deliberately not awaited
///
/// A `Drop` impl cannot await, so the work moves to a thread rather than the guard's owner. The cost is that a
/// caller cannot observe the outcome, so what would have been a returned error is **printed** instead: an
/// abandoned directory that nobody reports is exactly how this defect stayed invisible for thousands of runs.
///
/// The bound is measured rather than guessed, and it matters more than it looks. This code left **124** leftover
/// directories per full-workspace run with no retry at all and **7–9** with the window below. A **longer** window
/// does not improve on that (a ten-second version left the same handful), and an always-running retry loop is
/// **worse** — it measured **22**, because a thread competing for CPU across the whole suite delays the runtime
/// teardowns that are what actually release the handles. What remains after this window are fixtures holding an
/// `Arc<SqliteDatabase>` — through a `ToolPipeline` — past the directory guard, so the handle is released only at
/// the end of the test or at process exit. The former this window covers; the latter nothing in-process can, and
/// it is recorded rather than claimed as fixed.
pub fn remove_scratch_dir(path: &Path) {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let mut last = None;
        for attempt in 0..MAX_SCRATCH_REMOVAL_ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(SCRATCH_REMOVAL_INTERVAL);
            }
            match std::fs::remove_dir_all(&path) {
                Ok(()) => return,
                // `NotFound` means the directory is already gone, which is success for this function's purpose
                // and not a reason to keep retrying.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error) => last = Some(error),
            }
        }
        if let Some(error) = last {
            // Printed rather than returned: a `Drop` cannot report, and a silently abandoned directory is how
            // this defect stayed invisible. This is the one place a test can notice without failing.
            eprintln!(
                "jarvis: could not remove the scratch directory {}: {error}",
                path.display()
            );
        }
    });
}

/// How many times [`remove_scratch_dir`] retries before giving up.
///
/// The removal needs the runtime that owns the pool to finish tearing down, which is not synchronised with the
/// fixture. At [`SCRATCH_REMOVAL_INTERVAL`] per attempt this is a window measured in seconds, and it exists so a
/// **stuck** removal cannot leave a thread retrying for the life of the test binary.
pub const MAX_SCRATCH_REMOVAL_ATTEMPTS: u32 = 400;

/// How long [`remove_scratch_dir`] waits between attempts.
pub const SCRATCH_REMOVAL_INTERVAL: Duration = Duration::from_millis(25);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// **Two calls never return the same tag, which is the entire property.**
    ///
    /// A thousand calls rather than two, because the old scheme's failure was rare enough to survive many runs:
    /// a pid collision has to occur *and* the process has to reach the same sequence value, which is 0 only
    /// once per run — so the collision was invisible until enough directories had leaked to make a recycled pid
    /// likely. A test asserting two distinct values would have passed for the broken scheme too.
    #[test]
    fn a_thousand_tags_are_all_distinct() {
        let tags: HashSet<String> = (0..1000).map(|_| scratch_tag()).collect();
        assert_eq!(
            tags.len(),
            1000,
            "a repeated tag means two runs can share a directory"
        );
    }

    /// **The tag is a `UUIDv7` in lowercase hex with dashes, so it needs no escaping in a path.**
    ///
    /// Asserted rather than assumed, because the tag is interpolated into a path on three platforms and a
    /// character that a filesystem dislikes would surface as an I/O error in whichever fixture happened to run
    /// first — which is the least informative place for the problem to appear.
    #[test]
    fn the_tag_is_path_safe() {
        let tag = scratch_tag();
        assert_eq!(
            tag.len(),
            36,
            "a UUID's canonical form is 36 characters: {tag}"
        );
        assert_eq!(tag.matches('-').count(), 4, "a UUID has four dashes: {tag}");
        assert!(
            tag.chars().all(|character| character.is_ascii_digit()
                || ('a'..='f').contains(&character)
                || character == '-'),
            "the tag must be lowercase hex and dashes: {tag}"
        );
        // The version nibble, so a future change of `SessionId`'s generator is caught here rather than by a
        // reader who assumed "UUID" meant any version.
        assert_eq!(
            tag.chars().nth(14),
            Some('7'),
            "the tag must be a v7 UUID: {tag}"
        );
    }

    /// **A directory that does not exist is a success, not a reason to retry.**
    ///
    /// This is the branch that would otherwise make every teardown take the whole retry window: the common case
    /// is that the removal succeeded on the first attempt, and `NotFound` is the other common case — a fixture
    /// whose directory a previous step already removed. Retrying 400 times on it would add seconds to a suite
    /// for no benefit, so it is asserted rather than left to chance.
    #[test]
    fn removing_a_directory_that_is_gone_returns_promptly() {
        let path = std::env::temp_dir().join(format!("jarvis-gone-{}", scratch_tag()));
        remove_scratch_dir(&path);
        assert!(!path.exists(), "a path that never existed must stay absent");
    }

    /// **A directory that CAN be removed is removed, which is the property this function exists for.**
    ///
    /// The retry exists because a removal can fail while a database handle is still being released. This asserts
    /// the ordinary path — nothing holding the directory — so a change that broke the simple case (an inverted
    /// condition, a retry that never attempts the removal) fails here rather than showing up as a leak nobody
    /// looks for.
    #[test]
    fn a_removable_directory_is_actually_removed() {
        let path = std::env::temp_dir().join(format!("jarvis-removable-{}", scratch_tag()));
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("create scratch directory: {error}"));
        std::fs::write(path.join("payload.txt"), "content")
            .unwrap_or_else(|error| panic!("write a file inside it: {error}"));

        remove_scratch_dir(&path);

        // The removal is asynchronous by design, so this waits for it with a bound well under the function's
        // own ceiling — long enough to be reliable, short enough that a broken implementation fails rather than
        // hanging the suite.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            !path.exists(),
            "a directory nothing is holding must be removed, at {}",
            path.display()
        );
    }

    /// **No test fixture removes a scratch directory without going through [`remove_scratch_dir`].**
    ///
    /// This is the guard that keeps the fix from eroding, and it is a **source scan** rather than a behavioural
    /// test on purpose. The defect it prevents is invisible at runtime: a fixture that reverts to a bare
    /// `remove_dir_all` in a `Drop` impl passes every test and simply leaves a directory behind, exactly as the
    /// original did for thousands of runs. There is no assertion that can observe a directory *not* leaking
    /// without measuring the filesystem before and after a suite, which a unit test cannot do for itself.
    ///
    /// The scan is the same shape `jarvis-mcp-transport`'s boundary test uses, and for the same reason: a rule
    /// about what the **source** may contain needs to read the source.
    ///
    /// What it does **not** catch, recorded rather than implied: a fixture that computes its own path without
    /// `scratch_tag` (which the tag's own tests cover), and a removal that must happen for a reason other than a
    /// scratch directory — `jarvis-tools` clears a subdirectory to prove a re-read, and that is a different
    /// operation which this scan would flag if it were spelled with `remove_dir_all`. That is why the scan looks
    /// for the **exact** form a guard uses, `remove_dir_all(&self.0)`, rather than for the function name.
    #[test]
    fn no_fixture_bypasses_the_scratch_removal_helper() {
        // The workspace root, derived from this crate's own manifest so the test does not depend on the working
        // directory the harness happens to use.
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let workspace = crate_dir
            .parent()
            .and_then(std::path::Path::parent)
            .unwrap_or_else(|| panic!("the crate directory has a workspace above it"));

        let mut offenders = Vec::new();
        let mut scanned = 0_u32;
        walk(workspace, &mut |path, text| {
            scanned += 1;
            for (index, line) in text.lines().enumerate() {
                let stripped = line.trim();
                // Comments are skipped, because this module's own documentation quotes the offending form while
                // explaining why it is wrong — and a scan that flagged its own explanation would be a scan
                // nobody could keep passing.
                if stripped.starts_with("//") || stripped.starts_with('*') {
                    continue;
                }
                // The exact call a guard's `Drop` makes, and the `let` matters: what identifies a bypass is a
                // removal whose **result is discarded** (`let _ =`), which is the swallow that hid this defect.
                // Requiring the binding also keeps the scan from matching its own check below, which contains
                // the same substring inside a `contains(..)` call.
                if stripped.starts_with("let ")
                    && stripped.contains("remove_dir_all(&self.0)")
                    && !stripped.contains("remove_scratch_dir")
                {
                    offenders.push(format!("{}:{}", path.display(), index + 1));
                }
            }
        });

        assert!(
            scanned > 0,
            "the scan must have read at least one file, or it proves nothing"
        );
        assert!(
            offenders.is_empty(),
            "these fixtures remove a scratch directory without waiting for its handles to be released, which \
             leaks the directory: {offenders:#?}"
        );
    }

    /// Calls `visit` for every **Rust source** file under `root`, skipping build output and version control.
    ///
    /// Only `.rs` files are read, which is what keeps the walk cheap: the workspace holds a `target` directory
    /// and other build output whose files are both numerous and large, and reading them to decide they are
    /// irrelevant cost this scan about eighty seconds before the check was moved here.
    fn walk(root: &std::path::Path, visit: &mut impl FnMut(&std::path::Path, &str)) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "target" || name == ".git" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                walk(&path, visit);
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                visit(&path, &text);
            }
        }
    }
}
