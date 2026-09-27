//! Test-only helpers shared by this crate's unit tests.
//!
//! # Why an extension trait rather than `expect`
//!
//! The workspace denies `clippy::expect_used` and `clippy::unwrap_used` in test code as well as in library
//! code, so a test that wants to say "this must have succeeded" cannot write `.expect("...")`. The
//! alternatives are worse than they look. An `#[allow]` at the call site silences the lint for *every*
//! `expect` in that function, including the ones that would have caught a real problem. A per-test
//! `unwrap_or_else(|error| panic!(...))` is the same length as the `expect` it replaces and repeats the
//! message shape at every call site.
//!
//! So the message lives in one place and the call sites read `.must("valid base url")`. The failure message
//! names the whole error rather than a fixed string, which is the difference between a test that fails with
//! "invalid base url" and one that fails with the reason.
//!
//! The success value deliberately carries **no `Debug` bound**: several of these call sites unpack a
//! `MutexGuard` or a response type that has no reason to be printable, and requiring one would make the
//! helper unusable exactly where it is most useful. The *error* is printed with `{:?}` for the same reason.

/// Unpacks a `Result` that must have succeeded, or panics with the error.
pub(crate) trait ResultMustExt<T, E> {
    /// Returns the success value, or panics naming `what` and the error.
    fn must(self, what: &str) -> T;
}

impl<T, E: std::fmt::Debug> ResultMustExt<T, E> for Result<T, E> {
    fn must(self, what: &str) -> T {
        match self {
            Ok(value) => value,
            Err(error) => panic!("{what}: {error:?}"),
        }
    }
}

/// Unpacks a `Result` that must have failed, or panics.
pub(crate) trait ResultMustErrExt<T, E> {
    /// Returns the error, or panics naming `what`.
    fn must_err(self, what: &str) -> E;
}

impl<T, E> ResultMustErrExt<T, E> for Result<T, E> {
    fn must_err(self, what: &str) -> E {
        match self {
            Ok(_) => panic!("{what}: expected a failure, but it succeeded"),
            Err(error) => error,
        }
    }
}
