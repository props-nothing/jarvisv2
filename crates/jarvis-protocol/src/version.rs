use thiserror::Error;

/// Protocol version implemented by this build.
pub const PROTOCOL_VERSION: u16 = 1;
/// Oldest protocol version this build can still serve.
pub const MIN_SUPPORTED_PROTOCOL: u16 = 1;
/// Newest protocol version this build can serve.
pub const MAX_SUPPORTED_PROTOCOL: u16 = 1;

/// Explains why a client and daemon could not agree on a protocol version.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum NegotiationError {
    /// The client sent an inverted version range.
    #[error("client protocol range is inverted: minimum {minimum} exceeds maximum {maximum}")]
    InvalidRange {
        /// Client-declared minimum protocol version.
        minimum: u16,
        /// Client-declared maximum protocol version.
        maximum: u16,
    },
    /// The client's newest version is older than anything the daemon serves.
    #[error(
        "client protocol maximum {client_maximum} is older than the supported minimum {supported_minimum}"
    )]
    ClientTooOld {
        /// Newest protocol the client supports.
        client_maximum: u16,
        /// Oldest protocol the daemon supports.
        supported_minimum: u16,
    },
    /// The client's oldest version is newer than anything the daemon serves.
    #[error(
        "client protocol minimum {client_minimum} is newer than the supported maximum {supported_maximum}"
    )]
    ClientTooNew {
        /// Oldest protocol the client supports.
        client_minimum: u16,
        /// Newest protocol the daemon supports.
        supported_maximum: u16,
    },
}

/// Selects the highest protocol version both peers can speak.
///
/// # Errors
///
/// Returns [`NegotiationError`] when the client range is inverted or has no
/// overlap with the daemon's supported window.
pub const fn negotiate(client_minimum: u16, client_maximum: u16) -> Result<u16, NegotiationError> {
    if client_minimum > client_maximum {
        return Err(NegotiationError::InvalidRange {
            minimum: client_minimum,
            maximum: client_maximum,
        });
    }
    if client_maximum < MIN_SUPPORTED_PROTOCOL {
        return Err(NegotiationError::ClientTooOld {
            client_maximum,
            supported_minimum: MIN_SUPPORTED_PROTOCOL,
        });
    }
    if client_minimum > MAX_SUPPORTED_PROTOCOL {
        return Err(NegotiationError::ClientTooNew {
            client_minimum,
            supported_maximum: MAX_SUPPORTED_PROTOCOL,
        });
    }
    Ok(if client_maximum < MAX_SUPPORTED_PROTOCOL {
        client_maximum
    } else {
        MAX_SUPPORTED_PROTOCOL
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_selects_the_highest_common_version() {
        assert_eq!(negotiate(1, 1), Ok(1));
        assert_eq!(negotiate(0, 5), Ok(MAX_SUPPORTED_PROTOCOL));
    }

    #[test]
    fn incompatible_or_inverted_ranges_fail_closed() {
        assert_eq!(
            negotiate(2, 1),
            Err(NegotiationError::InvalidRange {
                minimum: 2,
                maximum: 1
            })
        );
        assert_eq!(
            negotiate(9, 12),
            Err(NegotiationError::ClientTooNew {
                client_minimum: 9,
                supported_maximum: MAX_SUPPORTED_PROTOCOL
            })
        );
        assert_eq!(
            negotiate(0, 0),
            Err(NegotiationError::ClientTooOld {
                client_maximum: 0,
                supported_minimum: MIN_SUPPORTED_PROTOCOL
            })
        );
    }
}
