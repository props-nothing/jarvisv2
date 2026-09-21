//! Acceptance-gate support crate.
//!
//! This package exists so the process-level gate is a first-class workspace
//! member rather than a loose script. `docs/architecture/repository-layout.md`
//! places full user journeys in `tests/e2e`, and `docs/development/testing.md`
//! requires platform behavior to be proven on native environments.
//!
//! The library target is intentionally empty: the gate is an integration test, and
//! nothing in this package may be depended on by shipped code.

#![forbid(unsafe_code)]

/// Marks the package as existing for its integration tests only.
#[must_use]
pub const fn acceptance_only() -> bool {
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_package_is_test_only() {
        assert!(super::acceptance_only());
    }
}
