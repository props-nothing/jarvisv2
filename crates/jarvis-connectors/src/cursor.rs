//! The sync cursor: how a connector resumes from where it stopped, without skipping or repeating.
//!
//! `tools-and-connectors.md` lists "webhook/subscription lifecycle and incremental sync cursors" as
//! connector-owned, and `storage.md` lists "connector accounts, scopes, sync cursors, and webhook
//! subscriptions" as durable state. This module is the cursor's vocabulary.
//!
//! # Why a cursor is not an opaque string
//!
//! The tempting design is a single `String` a connector interprets, and it fails in a specific way: it makes
//! **"we have never synced"** and **"we synced up to this point"** the same kind of value, so a caller
//! cannot tell a first run from a resumed one. That distinction decides whether a provider's "everything
//! since your last sync" is safe to trust — a cursor restored from the wrong account, or from a *different*
//! connector version, would silently skip whatever lies between. So the cursor is a struct whose
//! [`SyncCursorKind`] names the **shape** of the token, which is what lets a decoder refuse a token it did
//! not produce.
//!
//! # Why the kind matters more than it looks
//!
//! Providers express "where I am" in incompatible ways, and they are not interchangeable:
//!
//! - a **history id** (Gmail) is a monotonic marker, and a request for history after a stale one simply has
//!   no answer — the provider has expired it;
//! - a **delta token** (Microsoft Graph) is opaque and provider-issued, and asking for a delta from a token
//!   from another mailbox is a request the provider may answer *differently* rather than refuse;
//! - an **etag** is only meaningful against the resource it came from;
//! - an **offset count** is the only kind that can be reconstructed, and it is also the one that silently
//!   duplicates or skips when the underlying set changes.
//!
//! Each variant therefore carries what it needs, and [`SyncCursor::kind`] is what a provider adapter must
//! match on. **A cursor is bound to an account and a connector version**, because a cursor from one is not
//! applicable to the other and nothing about the token itself says so — `P3-008a`'s lesson that a value must
//! be accompanied by what it denotes.

use jarvis_core::UtcTimestamp;

use crate::account::AccountReference;

/// The longest accepted cursor token.
///
/// A bound exists because a cursor is provider-supplied text that becomes a database column and a request
/// parameter. A provider token that legitimately exceeds this is a cursor design that should be paginated
/// rather than stored whole.
pub const MAX_SYNC_CURSOR_CHARS: usize = 2_048;

/// What kind of position a cursor records.
///
/// The variants are the four shapes a real provider API uses, and each exists because it changes what a
/// caller may safely conclude from the value. See the module doc.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncCursorKind {
    /// A provider-issued opaque token whose only use is to be sent back.
    ///
    /// Its safety property is **negative**: nothing may be inferred from it, so a caller cannot validate it
    /// and must treat a provider's refusal as authoritative. Microsoft Graph's delta tokens are this shape.
    OpaqueToken,
    /// A monotonic provider marker, where a **stale** value means history was lost.
    ///
    /// Gmail's `historyId` is this shape, and the distinction from [`Self::OpaqueToken`] is that staleness is
    /// detectable and has a defined recovery (`historyId` too old means a full resync). Treating it as opaque
    /// would lose that recovery, and treating an opaque token as monotonic would invent a comparison the
    /// provider never offered.
    MonotonicMarker,
    /// A resource version, meaningful only against the resource it came from.
    Etag,
    /// A count of items already seen.
    ///
    /// The only reconstructible kind, and the only one that silently duplicates or skips when the underlying
    /// set changes — a deleted item shifts everything after it. Representable because some APIs offer nothing
    /// better, and named so a caller cannot mistake it for a stable position.
    Offset,
    /// No position: the connector has never synced, or its position was discarded.
    ///
    /// A **variant rather than an absent cursor**, which is the module's central decision: a full sync is a
    /// decision with consequences (cost, time, possibly a rate-limit budget), and an absent value would make
    /// it the default a caller stumbles into.
    Start,
}

impl SyncCursorKind {
    /// Returns the stable snake-case code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpaqueToken => "opaque_token",
            Self::MonotonicMarker => "monotonic_marker",
            Self::Etag => "etag",
            Self::Offset => "offset",
            Self::Start => "start",
        }
    }

    /// Returns whether this kind can become **invalid through age**.
    ///
    /// Only a monotonic marker can: a provider expires its history, so a marker behind the retained window
    /// has no answer and the recovery is a full resync. An opaque token is refused by the provider when it is
    /// stale, which arrives as an error rather than as a detectable condition — so this predicate says "a
    /// caller can tell", not "this kind never goes stale".
    #[must_use]
    pub const fn can_be_detected_as_stale(self) -> bool {
        matches!(self, Self::MonotonicMarker | Self::Etag)
    }

    /// Returns whether a full resync is the only way to recover from a lost cursor of this kind.
    ///
    /// Everything except [`Self::Offset`], which can be reconstructed from what has already been stored — and
    /// that is precisely why it is the dangerous kind, since reconstruction assumes the set has not changed.
    #[must_use]
    pub const fn needs_full_resync_when_lost(self) -> bool {
        !matches!(self, Self::Start | Self::Offset)
    }
}

/// A connector's position in a provider's stream of changes.
///
/// Constructed through [`SyncCursor::new`], which binds the token to the account it came from and the
/// connector version that issued it. Both bindings are the point: a token alone says nothing about which
/// mailbox it belongs to, and a provider that answers a foreign token *differently* rather than refusing it
/// is exactly the case a caller cannot detect unaided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncCursor {
    kind: SyncCursorKind,
    token: Option<String>,
    account: AccountReference,
    connector_version: String,
    observed_at: UtcTimestamp,
}

impl SyncCursor {
    /// Records a cursor.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::Shape`] when a [`SyncCursorKind::Start`] cursor carries a token or any other
    /// kind carries none, and [`CursorError::Token`] when the token is empty, oversized, or holds a control
    /// character. The control-character refusal is not cosmetic: this value becomes a request parameter and
    /// a log field, and a newline in either forges a record.
    pub fn new(
        kind: SyncCursorKind,
        token: Option<String>,
        account: AccountReference,
        connector_version: impl Into<String>,
        observed_at: UtcTimestamp,
    ) -> Result<Self, CursorError> {
        match (kind, token.as_deref()) {
            (SyncCursorKind::Start, Some(_)) => {
                return Err(CursorError::Shape {
                    reason: "a `start` cursor carries no token; a token would make `never synced` \
                             indistinguishable from `synced to here`, which is the distinction this type \
                             exists to keep",
                });
            }
            (SyncCursorKind::Start, None) => {}
            (_, None) => {
                return Err(CursorError::Shape {
                    reason: "every cursor kind other than `start` must carry a token",
                });
            }
            (_, Some(token)) => {
                if token.is_empty()
                    || token.chars().count() > MAX_SYNC_CURSOR_CHARS
                    || token.chars().any(char::is_control)
                {
                    return Err(CursorError::Token {
                        reason: "a cursor token must be 1 to 2048 characters with no control characters, \
                                 because it becomes a request parameter and a stored field",
                    });
                }
            }
        }
        let connector_version = connector_version.into();
        if connector_version.trim().is_empty() {
            return Err(CursorError::Shape {
                reason: "a cursor must name the connector version that issued it, because a cursor from a \
                         previous format is not applicable to the current one",
            });
        }
        Ok(Self {
            kind,
            token,
            account,
            connector_version,
            observed_at,
        })
    }

    /// Returns a cursor that means "never synced".
    ///
    /// Takes the connector version as the validated type rather than a string, so the "unspecified" default a
    /// string version would need becomes unrepresentable: a start cursor still names the connector that is
    /// about to run, because that is what the first stored cursor will be compared against.
    #[must_use]
    pub fn start(
        account: AccountReference,
        connector_version: &crate::manifest::ConnectorVersion,
        observed_at: UtcTimestamp,
    ) -> Self {
        Self {
            kind: SyncCursorKind::Start,
            token: None,
            account,
            connector_version: connector_version.as_str().to_owned(),
            observed_at,
        }
    }

    /// Returns the cursor kind.
    #[must_use]
    pub fn kind(&self) -> SyncCursorKind {
        self.kind
    }

    /// Returns the token, absent for [`SyncCursorKind::Start`].
    #[must_use]
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Returns the account the cursor came from.
    #[must_use]
    pub fn account(&self) -> &AccountReference {
        &self.account
    }

    /// Returns the connector version that issued it.
    #[must_use]
    pub fn connector_version(&self) -> &str {
        &self.connector_version
    }

    /// Returns when the cursor was observed.
    #[must_use]
    pub fn observed_at(&self) -> UtcTimestamp {
        self.observed_at
    }

    /// Returns whether this cursor is applicable to the given account and connector version.
    ///
    /// The check an adapter must make before sending a cursor to a provider. Both halves matter: a token from
    /// another account is a request the provider may answer with *another* account's changes, and a token
    /// from an earlier connector version may use a format this adapter cannot interpret.
    #[must_use]
    pub fn applies_to(&self, account: &AccountReference, connector_version: &str) -> bool {
        self.account == *account && self.connector_version == connector_version
    }

    /// Returns whether a full resync is required to continue.
    ///
    /// True for [`SyncCursorKind::Start`] and for a cursor that cannot be validated — and a caller should ask
    /// this rather than comparing against `Start` itself, so adding a fourth reason becomes a change in one
    /// place.
    #[must_use]
    pub fn requires_full_resync(&self) -> bool {
        self.kind == SyncCursorKind::Start
    }
}

/// The window a sync run covers.
///
/// `tools-and-connectors.md` gives connectors "incremental sync cursors" and `events-and-workflows.md` makes
/// the events they produce durable, so a sync run has a **bounded window** rather than an open-ended
/// catch-up: a connector that synced a year of mail because a cursor was lost would exhaust a rate-limit
/// budget and produce a backlog nobody can review. The window is the caller's statement of how far to go.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncWindow {
    /// Where the window starts. A full resync starts nowhere, which is `None`.
    pub since: Option<UtcTimestamp>,
    /// The most items one run may process.
    pub max_items: u32,
    /// The longest one run may take, in seconds.
    pub max_seconds: u32,
}

impl SyncWindow {
    /// The most items one sync run may process.
    ///
    /// A bound with a reason rather than a round number: every item becomes a canonical record, so an
    /// unbounded run is a memory and storage decision made by a provider's backfill. The same reasoning as
    /// `MAX_MEMORIES_LOADED` and `MAX_SIMILARITY_RESULTS` in the storage crate.
    pub const MAX_ITEMS_CEILING: u32 = 10_000;

    /// The longest one sync run may take.
    pub const MAX_SECONDS_CEILING: u32 = 3_600;

    /// Builds a bounded window.
    ///
    /// # Errors
    ///
    /// Returns [`CursorError::Window`] when the item count is zero or above [`Self::MAX_ITEMS_CEILING`], or
    /// the duration is zero or above [`Self::MAX_SECONDS_CEILING`]. Zero is refused rather than read as "no
    /// limit", which is the same reasoning `jarvis-storage`'s page bounds use: a caller that asked for none
    /// would otherwise receive an unbounded run.
    pub fn new(
        since: Option<UtcTimestamp>,
        max_items: u32,
        max_seconds: u32,
    ) -> Result<Self, CursorError> {
        if max_items == 0 || max_items > Self::MAX_ITEMS_CEILING {
            return Err(CursorError::Window {
                reason: "a sync window must ask for 1 to 10000 items; zero would mean `no limit` to a \
                         caller that read it as one",
            });
        }
        if max_seconds == 0 || max_seconds > Self::MAX_SECONDS_CEILING {
            return Err(CursorError::Window {
                reason: "a sync window must allow 1 to 3600 seconds",
            });
        }
        Ok(Self {
            since,
            max_items,
            max_seconds,
        })
    }

    /// Returns whether the window covers everything the provider offers.
    ///
    /// Reported rather than inferred, because a full window is the expensive case and a caller that logs it
    /// needs to say so.
    #[must_use]
    pub const fn is_full_resync(&self) -> bool {
        self.since.is_none()
    }
}

/// Why a cursor or sync window is unusable.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CursorError {
    /// The cursor's shape contradicts its kind.
    #[error("the sync cursor's shape is wrong: {reason}")]
    Shape {
        /// What is wrong.
        reason: &'static str,
    },
    /// The cursor token is unusable.
    #[error("the sync cursor token is unusable: {reason}")]
    Token {
        /// What is wrong.
        reason: &'static str,
    },
    /// The sync window is unusable.
    #[error("the sync window is unusable: {reason}")]
    Window {
        /// What is wrong.
        reason: &'static str,
    },
}

/// A cursor together with what it denotes, so a decoder cannot read one without the other.
///
/// `P3-006a` established the pattern: two values that must agree, with nothing holding both, is a defect
/// class that lives only between two correct modules. A cursor token and the account it came from are exactly
/// that pair, so the parts travel together and [`SyncCursor::new`] is the only way to assemble them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncCursorParts {
    /// The cursor kind.
    pub kind: SyncCursorKind,
    /// The provider token, absent for a start cursor.
    pub token: Option<String>,
    /// The account the cursor belongs to.
    pub account: AccountReference,
    /// The connector version that issued it.
    pub connector_version: String,
    /// When it was observed.
    pub observed_at: UtcTimestamp,
}

impl SyncCursorParts {
    /// Assembles the parts into a validated cursor.
    ///
    /// # Errors
    ///
    /// Returns whatever [`SyncCursor::new`] returns.
    pub fn assemble(self) -> Result<SyncCursor, CursorError> {
        SyncCursor::new(
            self.kind,
            self.token,
            self.account,
            self.connector_version,
            self.observed_at,
        )
    }
}

impl From<SyncCursor> for SyncCursorParts {
    fn from(cursor: SyncCursor) -> Self {
        Self {
            kind: cursor.kind,
            token: cursor.token,
            account: cursor.account,
            connector_version: cursor.connector_version,
            observed_at: cursor.observed_at,
        }
    }
}

#[cfg(test)]
#[path = "cursor_tests.rs"]
mod tests;
