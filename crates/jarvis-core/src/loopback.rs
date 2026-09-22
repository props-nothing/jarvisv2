//! Loopback HTTP endpoint policy for the daemon's peer HTTP transport.
//!
//! ADR-0011 makes `/api/v1` a first-class peer transport to local IPC, bound to loopback by
//! default, with anything outside loopback deferred to remote mode as explicit, TLS-terminated,
//! separately configured work. This module is the **client half** of that rule.
//!
//! # The host is unrepresentable, not validated
//!
//! [`LoopbackHost`] has no address field: it carries a port and derives the address from
//! [`LOOPBACK_ADDRESS`]. There is therefore no constructor, argument, or configuration value
//! that can point a client at a non-loopback host, and no check a later code path could skip.
//! A type with a validated `host: String` field would be one deref away from sending the
//! profile credential to an address nobody intended, and the mitigation for that is a rule
//! someone has to remember rather than a shape the compiler enforces.
//!
//! This mirrors the reason `Endpoints::grpc_endpoint` in an unrelated project returns a
//! key-free string: making a mistake structurally impossible beats documenting that it is
//! discouraged.

use std::fmt;

use thiserror::Error;

/// The only address the daemon's HTTP transport binds and a client may target.
pub const LOOPBACK_ADDRESS: &str = "127.0.0.1";

/// A validated loopback HTTP endpoint.
///
/// Holds a port only. See the module documentation for why the host is not a field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoopbackHost {
    port: u16,
}

impl LoopbackHost {
    /// Creates an endpoint for a loopback port.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidLoopbackHost::PortIsZero`] for port 0. Port 0 asks the operating
    /// system for an ephemeral port, which gives a client no port it could name, so the same
    /// value that makes a *server* bindable makes a *client* unusable. The daemon refuses it
    /// for the same reason (`jarvis_storage::Config::validate`).
    pub const fn new(port: u16) -> Result<Self, InvalidLoopbackHost> {
        if port == 0 {
            return Err(InvalidLoopbackHost::PortIsZero);
        }
        Ok(Self { port })
    }

    /// Returns the port.
    #[must_use]
    pub const fn port(self) -> u16 {
        self.port
    }

    /// Returns the `host:port` authority.
    #[must_use]
    pub fn authority(self) -> String {
        format!("{LOOPBACK_ADDRESS}:{}", self.port)
    }

    /// Returns an absolute URL for a path on this endpoint.
    ///
    /// The scheme is fixed to `http`, which is correct rather than lax: loopback traffic never
    /// leaves the machine, and remote mode — the only case where TLS is required — is a
    /// separate transport that this type deliberately cannot express.
    #[must_use]
    pub fn url(self, path: &str) -> String {
        format!("http://{}{path}", self.authority())
    }
}

impl fmt::Display for LoopbackHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.authority())
    }
}

/// Why a loopback endpoint could not be created.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum InvalidLoopbackHost {
    /// Port 0 requests an ephemeral port, which a client cannot name.
    #[error("the loopback port must be between 1 and 65535, not 0")]
    PortIsZero,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_becomes_a_loopback_authority_and_url() {
        let host =
            LoopbackHost::new(8765).unwrap_or_else(|error| panic!("8765 must be usable: {error}"));
        assert_eq!(host.port(), 8765);
        assert_eq!(host.authority(), "127.0.0.1:8765");
        assert_eq!(
            host.url("/api/v1/runs"),
            "http://127.0.0.1:8765/api/v1/runs"
        );
        assert_eq!(host.to_string(), "127.0.0.1:8765");
    }

    /// Port 0 is refused because an ephemeral port cannot be named by a client. This is the
    /// same value `Config::validate` rejects for `daemon.http_port`, so a server that refuses
    /// to bind it and a client that refuses to target it agree on one rule.
    #[test]
    fn an_unnamable_port_is_refused() {
        assert_eq!(
            LoopbackHost::new(0),
            Err(InvalidLoopbackHost::PortIsZero),
            "port 0 asks for an ephemeral port that no client could name"
        );
    }

    /// The type cannot express another host, so every URL it can build is on loopback. This is
    /// the property the credential's safety depends on, asserted rather than assumed.
    #[test]
    fn every_buildable_url_stays_on_loopback() {
        for port in [1_u16, 80, 8765, 65_535] {
            let host = LoopbackHost::new(port)
                .unwrap_or_else(|error| panic!("{port} must be usable: {error}"));
            let url = host.url("/api/v1/runs");
            assert!(
                url.starts_with("http://127.0.0.1:"),
                "{url} must address loopback only"
            );
        }
    }
}
