//! Which protocol revision a connection negotiates, and why it is never left to the SDK's default.
//!
//! `P3-007` established that `rmcp`'s `ProtocolVersion::LATEST` is `V_2025_11_25` — the **legacy**
//! era, the one with the `initialize` handshake — while protocol revision `2026-07-28` is the one
//! this project targets and is a rewrite that *removed* that handshake. The SDK's own README calls
//! `LATEST` "newest stable version this SDK defaults to" and, in the same file, says the SDK
//! implements the stable `2026-07-28` specification. **The two statements cannot both be read the
//! same way, so the constant is checked rather than trusted.**
//!
//! `MODERN_REVISION` is the revision this crate will only ever ask for. `sdk_default_revision` and
//! the test that compares them exist so that an SDK bump which makes `LATEST` mean `2026-07-28`
//! produces a *failing test and a decision* rather than a silent change in which era we speak.

use rmcp::model::ProtocolVersion;

/// The protocol revision JARVIS speaks to modern MCP servers.
///
/// Named as a string literal rather than borrowed from the SDK, because this is a fact about the
/// *protocol* and not about the SDK. If the SDK ever stops knowing this revision, the compile of
/// [`modern_revision`] fails, which is the honest outcome — a silent fallback would negotiate a
/// different era while appearing to target this one.
pub const MODERN_REVISION: &str = "2026-07-28";

/// Returns the SDK constant for [`MODERN_REVISION`].
///
/// # Panics
///
/// Panics at runtime if the pinned SDK no longer defines this revision. That is deliberate: the
/// alternative is a silent downgrade to a legacy handshake, which is the defect this module exists
/// to prevent. The version is pinned exactly in the workspace manifest, so this cannot change under
/// a `cargo update` — only under a deliberate bump, which must then re-read `P3-007`.
#[must_use]
pub fn modern_revision() -> ProtocolVersion {
    // Matched against the value, not the constant name, so a rename inside the SDK is caught here
    // rather than resolved by the compiler to some other revision.
    let candidate = ProtocolVersion::V_2026_07_28;
    assert_eq!(
        candidate.as_str(),
        MODERN_REVISION,
        "the pinned SDK's V_2026_07_28 is not {MODERN_REVISION}; re-read docs/research/integrations/mcp.md"
    );
    candidate
}

/// Returns whatever the pinned SDK currently calls its default preferred revision.
///
/// This is **observational only**: nothing negotiates with it. It is exposed so a test can assert it
/// is *not* [`MODERN_REVISION`], which documents the trap as an executable fact and turns an SDK
/// change into a decision rather than a surprise.
#[must_use]
pub fn sdk_default_revision() -> ProtocolVersion {
    ProtocolVersion::LATEST
}

/// Reports whether the pinned SDK's default is the modern revision.
///
/// `false` is the expected and currently true answer, and it is worth being able to assert rather
/// than assume.
#[must_use]
pub fn sdk_default_is_modern() -> bool {
    sdk_default_revision().as_str() == MODERN_REVISION
}

/// Names the revision a connection negotiated, for a log line or a stored observation.
///
/// Takes the SDK's type so a caller cannot pass a bare string that disagrees with what was actually
/// negotiated.
#[must_use]
pub fn describe_negotiated(version: &ProtocolVersion) -> String {
    let revision = version.as_str();
    if revision == MODERN_REVISION {
        format!("mcp {revision} (stateless: per-request metadata, no initialize handshake)")
    } else {
        format!(
            "mcp {revision} (LEGACY era: initialize handshake; JARVIS asked for {MODERN_REVISION})"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The trap, asserted rather than remembered.** If this fails after an SDK bump, `LATEST` has
    /// become the modern revision: the comment above is then stale and the negotiation strategy can
    /// be simplified — but it must be a decision, not an accident.
    #[test]
    fn the_sdk_default_revision_is_not_the_modern_one() {
        let default = sdk_default_revision();
        assert_ne!(
            default.as_str(),
            MODERN_REVISION,
            "the pinned SDK's LATEST is now {MODERN_REVISION}; re-read P3-007 before removing the explicit revision"
        );
        assert_eq!(default.as_str(), "2025-11-25");
        assert!(!sdk_default_is_modern());
    }

    /// The revision we ask for is expressible through the pinned SDK, which is the precondition for
    /// asking for it at all.
    #[test]
    fn the_modern_revision_exists_in_the_pinned_sdk() {
        assert_eq!(modern_revision().as_str(), MODERN_REVISION);
    }

    /// A log line must not describe a legacy negotiation as modern. The two eras are not
    /// interchangeable: one has a handshake and a session, the other has neither.
    #[test]
    fn a_description_distinguishes_the_two_eras() {
        let modern = describe_negotiated(&modern_revision());
        assert!(modern.contains("stateless"), "{modern}");
        assert!(!modern.contains("LEGACY"), "{modern}");

        let legacy = describe_negotiated(&sdk_default_revision());
        assert!(legacy.contains("LEGACY"), "{legacy}");
        assert!(
            legacy.contains(MODERN_REVISION),
            "a legacy negotiation should say what was asked for: {legacy}"
        );
    }
}
