//! A connector account: the identity a grant belongs to, verified by the provider.
//!
//! `identity-and-workspaces.md` lists "connector account: verified identity at an external provider" as one
//! of the identity kinds, and `tools-and-connectors.md` requires "account identity verified from the
//! provider, not user-entered labels". This module is that value, and the second requirement is what shapes
//! it: there is **no field for a label the caller supplied**, so an account cannot be named by whoever
//! created it.
//!
//! That matters because a user-entered label is a value an attacker chooses. `P3-009g` found the same defect
//! in MCP's `clientInfo` — "a self-reported name is evidence or nothing, never a permit" — and here a
//! user-chosen account label would be worse than a permit: it would be the thing an approval displays, so a
//! label reading "work calendar" could name a mailbox an attacker controls while the preview looked exactly
//! right. So [`VerifiedAccount`] carries the provider's own identifiers and a **display name the provider
//! reported**, and [`AccountReference`] — what the rest of the platform stores — carries a JARVIS-assigned
//! opaque identifier rather than any of them.

use std::fmt;

use jarvis_core::UtcTimestamp;

use crate::auth::AuthMethod;

/// The longest provider account identifier accepted.
pub const MAX_PROVIDER_ACCOUNT_ID_CHARS: usize = 256;

/// The longest display name accepted.
pub const MAX_ACCOUNT_DISPLAY_NAME_CHARS: usize = 100;

/// A JARVIS-assigned identifier for one connected account.
///
/// Opaque and stable, and deliberately **not** the provider's own identifier: the provider's value is what a
/// request is addressed with, so storing it as the account's key would make a provider-side change of that
/// value silently orphan every reference to it — and would put a provider-controlled string into paths,
/// logs, and diagnostics. `P3-009g`'s `Fingerprint` made the same move for the same reason.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AccountReference(String);

impl AccountReference {
    /// The longest accepted reference.
    pub const MAX_BYTES: usize = 64;

    /// Records a reference.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ConnectorError::Identifier`] when the value is unusable, reusing the connector
    /// identifier's alphabet because an account reference appears in the same places one does — a
    /// configuration key, a diagnostics field, a path segment.
    pub fn new(value: impl Into<String>) -> Result<Self, crate::ConnectorError> {
        let value = value.into();
        if value.is_empty() || value.len() > Self::MAX_BYTES {
            return Err(crate::ConnectorError::Identifier {
                value,
                reason: "an account reference must be 1 to 64 characters",
            });
        }
        if !value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        }) {
            return Err(crate::ConnectorError::Identifier {
                value,
                reason: "an account reference may hold only lowercase ASCII letters, digits, and `-`, \
                         because it appears in configuration keys, diagnostics fields, and path segments",
            });
        }
        Ok(Self(value))
    }

    /// Returns the reference text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What the provider said about an account, after a successful connection.
///
/// Every field is **the provider's** statement. None is user-supplied, which is the requirement's own
/// wording ("verified from the provider, not user-entered labels"), and the absence of a `label` field is
/// how that is enforced rather than merely documented.
#[derive(Clone, Eq, PartialEq)]
pub struct VerifiedAccount {
    reference: AccountReference,
    provider_account_id: String,
    display_name: Option<String>,
    method: AuthMethod,
    scopes: Vec<String>,
    verified_at: UtcTimestamp,
}

impl VerifiedAccount {
    /// Records a provider-verified account.
    ///
    /// # Errors
    ///
    /// Returns [`crate::ConnectorError::Identifier`] when the provider account identifier is empty,
    /// oversized, or holds a control character — the last because this value reaches logs, and a newline in
    /// a log field forges a line. Returns it for an oversized display name too, and a display name is
    /// *optional* because a provider may report none; what is refused is a **supplied but unusable** one.
    pub fn new(
        reference: AccountReference,
        provider_account_id: impl Into<String>,
        display_name: Option<String>,
        method: AuthMethod,
        scopes: Vec<String>,
        verified_at: UtcTimestamp,
    ) -> Result<Self, crate::ConnectorError> {
        let provider_account_id = provider_account_id.into();
        // A whitespace-only identifier is refused as well as an empty one: it satisfies a `NOT NULL` column
        // while denoting nothing, and a provider that returned only spaces is not a provider that answered.
        // Found by a test asserting the empty case, which passed while `"  "` did not — the two are the same
        // mistake and only one was checked.
        if provider_account_id.trim().is_empty()
            || provider_account_id.chars().count() > MAX_PROVIDER_ACCOUNT_ID_CHARS
            || provider_account_id.chars().any(char::is_control)
        {
            return Err(crate::ConnectorError::Identifier {
                value: provider_account_id,
                reason: "a provider account identifier must be 1 to 256 characters with no control \
                         characters and not only whitespace, because it reaches log fields where a newline \
                         would forge a line",
            });
        }
        if let Some(name) = display_name.as_deref()
            && (name.trim().is_empty() || name.chars().count() > MAX_ACCOUNT_DISPLAY_NAME_CHARS)
        {
            return Err(crate::ConnectorError::Identifier {
                value: name.to_owned(),
                reason: "a provider display name, when present, must be 1 to 100 characters",
            });
        }
        Ok(Self {
            reference,
            provider_account_id,
            display_name,
            method,
            scopes,
            verified_at,
        })
    }

    /// Returns the JARVIS-assigned reference.
    #[must_use]
    pub fn reference(&self) -> &AccountReference {
        &self.reference
    }

    /// Returns the provider's own account identifier.
    ///
    /// Exposed so an adapter can address the account, and **not** used as a key anywhere: see
    /// [`AccountReference`]. `Debug` is hand-written to redact it, because a provider account identifier is
    /// usually an email address and a diagnostic that printed it would leak the account's owner.
    #[must_use]
    pub fn provider_account_id(&self) -> &str {
        &self.provider_account_id
    }

    /// Returns the provider's display name, when it reported one.
    #[must_use]
    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    /// Returns how the account was connected.
    #[must_use]
    pub fn method(&self) -> AuthMethod {
        self.method
    }

    /// Returns the scopes the provider granted.
    #[must_use]
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    /// Returns when the provider confirmed the identity.
    #[must_use]
    pub fn verified_at(&self) -> UtcTimestamp {
        self.verified_at
    }

    /// Returns whether every one of `required` is granted.
    ///
    /// The predicate an operation's availability depends on: an operation whose scopes are not all granted
    /// must report **unavailable** rather than failing when called, so `P3-002`'s restrictive-wins rule
    /// applies one layer up. A plain set containment, because the scopes are the provider's own strings and
    /// JARVIS has no hierarchy over them.
    #[must_use]
    pub fn grants_all(&self, required: &[String]) -> bool {
        required.iter().all(|scope| self.scopes.contains(scope))
    }

    /// Returns the scopes in `required` that are **not** granted.
    ///
    /// The complement of [`Self::grants_all`], reported rather than inferred: the requirement says reauth
    /// must preserve account references "without hiding lost scopes", so a caller displaying a reauth prompt
    /// needs to name what is missing rather than only that something is.
    #[must_use]
    pub fn missing_scopes(&self, required: &[String]) -> Vec<String> {
        required
            .iter()
            .filter(|scope| !self.scopes.contains(scope))
            .cloned()
            .collect()
    }
}

impl fmt::Debug for VerifiedAccount {
    /// Redacts the provider account identifier, which is usually a person's address.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedAccount")
            .field("reference", &self.reference)
            .field("provider_account_id", &"<redacted>")
            .field("display_name", &self.display_name)
            .field("method", &self.method)
            .field("scopes", &self.scopes.len())
            .field("verified_at", &self.verified_at)
            .finish()
    }
}

/// Where an account stands.
///
/// `security.md`: "Deny overrides allow. Missing or stale evidence fails closed." So the variants that
/// permit work are the narrow ones, and every other state is represented rather than folded into a
/// `bool` — an account that needs a user to act requires a different response from one that is rate-limited,
/// and a boolean would make them read the same.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountStatus {
    /// Connected, and the granted scopes cover everything the connector declares.
    Connected,
    /// Connected, but one or more declared scopes were not granted.
    ///
    /// Distinct from [`Self::NeedsReauth`] because the remedy differs: a scope shortfall means the user
    /// declined something, which an operations list can route around, while a reauth means nothing works.
    ScopeShortfall,
    /// The refresh token expired or was revoked, so the user must reconnect.
    NeedsReauth,
    /// The provider is refusing requests, typically a rate limit or an outage.
    Unavailable,
    /// The account was disconnected deliberately.
    Disconnected,
}

impl AccountStatus {
    /// Returns whether any operation may run.
    #[must_use]
    pub const fn permits_calls(self) -> bool {
        matches!(self, Self::Connected | Self::ScopeShortfall)
    }

    /// Returns whether a user must act to restore service.
    #[must_use]
    pub const fn needs_user(self) -> bool {
        matches!(self, Self::NeedsReauth)
    }

    /// Returns whether the account is connected at all.
    ///
    /// Deliberately not the same as [`Self::permits_calls`]: an account in `ScopeShortfall` is connected and
    /// may run the operations its grant covers. Conflating the two would make a partial grant behave like a
    /// disconnect, which is the over-blocking direction `P4-008` recorded as the safe one to *fail* — but
    /// here the safe direction for a **shortfall** is to run what is granted, because refusing everything
    /// would make a user's deliberate partial consent indistinguishable from revocation.
    #[must_use]
    pub const fn is_connected(self) -> bool {
        matches!(
            self,
            Self::Connected | Self::ScopeShortfall | Self::Unavailable
        )
    }
}

#[cfg(test)]
#[path = "account_tests.rs"]
mod tests;
