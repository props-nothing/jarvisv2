use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Supported log verbosity levels.
///
/// This lives in the domain crate because it is stable, provider-neutral
/// vocabulary shared by configuration, logging setup, and diagnostics. It is
/// ordered from most to least verbose so a level can be compared.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Highly detailed diagnostic events.
    Trace,
    /// Developer-oriented diagnostic events.
    Debug,
    /// Normal lifecycle and operation events.
    #[default]
    Info,
    /// Degraded behavior that does not stop the process.
    Warn,
    /// Failed operations requiring attention.
    Error,
}

impl LogLevel {
    /// Returns the stable lowercase name used in configuration and logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for LogLevel {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_names_round_trip_and_order_from_verbose_to_quiet() {
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ] {
            assert_eq!(level.to_string().parse::<LogLevel>(), Ok(level));
        }

        assert!(LogLevel::Trace < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Error);
        assert_eq!(LogLevel::default(), LogLevel::Info);
        assert_eq!("verbose".parse::<LogLevel>(), Err(()));
    }
}
