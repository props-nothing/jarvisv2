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
}
