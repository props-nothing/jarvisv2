//! Stable human and JSON rendering for the command-line client.

use std::process::ExitCode;

use jarvis_core::ErrorCode;

/// Process exit status with documented, machine-readable meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExitStatus {
    /// The command succeeded.
    Ok,
    /// The command line was malformed.
    Usage,
    /// The daemon could not be reached or is not ready.
    Unavailable,
    /// Authentication or authorization failed.
    Denied,
    /// The request was invalid or conflicted.
    Rejected,
    /// Doctor found a degraded state with no blocking failure.
    DoctorWarnings,
    /// Doctor found at least one blocking failure.
    DoctorFailed,
    /// A requested repair did not verify.
    RepairFailed,
    /// An unexpected internal failure occurred.
    Internal,
}

impl ExitStatus {
    /// Maps a stable protocol error category to an exit status.
    pub(crate) const fn from_code(code: ErrorCode) -> Self {
        match code {
            ErrorCode::Authentication | ErrorCode::Authorization => Self::Denied,
            ErrorCode::Validation | ErrorCode::Conflict => Self::Rejected,
            ErrorCode::UnavailableCapability
            | ErrorCode::Unsupported
            | ErrorCode::RateLimited
            | ErrorCode::Timeout
            | ErrorCode::TransientUpstream
            | ErrorCode::PermanentUpstream
            | ErrorCode::ApprovalRequired
            | ErrorCode::AmbiguousEffect
            | ErrorCode::Cancelled => Self::Unavailable,
            ErrorCode::Internal => Self::Internal,
        }
    }
}

impl From<ExitStatus> for ExitCode {
    fn from(status: ExitStatus) -> Self {
        match status {
            ExitStatus::Ok => Self::SUCCESS,
            ExitStatus::Usage => Self::from(2),
            ExitStatus::Unavailable => Self::from(3),
            ExitStatus::Denied => Self::from(4),
            ExitStatus::Rejected => Self::from(5),
            ExitStatus::DoctorWarnings => Self::from(6),
            ExitStatus::DoctorFailed => Self::from(7),
            ExitStatus::RepairFailed => Self::from(8),
            ExitStatus::Internal => Self::from(1),
        }
    }
}

/// An ordered, secret-free set of `key=value` fields for one reply.
#[derive(Debug)]
pub(crate) struct Fields {
    kind: &'static str,
    entries: Vec<(String, String)>,
}

impl Fields {
    /// Starts a field set for one reply kind.
    pub(crate) const fn new(kind: &'static str) -> Self {
        Self {
            kind,
            entries: Vec::new(),
        }
    }

    /// Appends a field.
    #[must_use]
    pub(crate) fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.entries.push((key.into(), value.into()));
        self
    }

    /// Renders a single stable JSON object.
    pub(crate) fn render_json(&self) {
        let mut object = serde_json::Map::new();
        object.insert(
            "kind".to_owned(),
            serde_json::Value::String(self.kind.to_owned()),
        );
        for (key, value) in &self.entries {
            object.insert(key.clone(), typed_value(key, value));
        }
        println!("{}", serde_json::Value::Object(object));
    }

    /// Renders one `key=value` line per field.
    pub(crate) fn render_human(&self) {
        let rendered: Vec<String> = self
            .entries
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        println!("{} {}", self.kind, rendered.join(" "));
    }
}

fn typed_value(key: &str, value: &str) -> serde_json::Value {
    match key {
        "live" | "ready" => serde_json::Value::Bool(value == "true"),
        "protocol_version" | "config_schema" | "database_schema" => {
            value.parse::<i64>().map_or_else(
                |_| serde_json::Value::String(value.to_owned()),
                serde_json::Value::from,
            )
        }
        _ => serde_json::Value::String(value.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_statuses_are_distinct_and_stable() {
        assert_eq!(ExitCode::from(ExitStatus::Ok), ExitCode::SUCCESS);
        assert_eq!(ExitCode::from(ExitStatus::Usage), ExitCode::from(2));
        assert_eq!(ExitCode::from(ExitStatus::Unavailable), ExitCode::from(3));
        assert_eq!(ExitCode::from(ExitStatus::Denied), ExitCode::from(4));
        assert_eq!(ExitCode::from(ExitStatus::Rejected), ExitCode::from(5));
        assert_eq!(
            ExitCode::from(ExitStatus::DoctorWarnings),
            ExitCode::from(6)
        );
        assert_eq!(ExitCode::from(ExitStatus::DoctorFailed), ExitCode::from(7));
        assert_eq!(ExitCode::from(ExitStatus::RepairFailed), ExitCode::from(8));
    }

    #[test]
    fn json_values_are_typed_rather_than_stringly() {
        assert_eq!(typed_value("live", "true"), serde_json::Value::Bool(true));
        assert_eq!(
            typed_value("config_schema", "1"),
            serde_json::Value::from(1_i64)
        );
        assert_eq!(
            typed_value("phase", "ready"),
            serde_json::Value::String("ready".to_owned())
        );
    }
}
