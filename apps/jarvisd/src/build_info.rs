use jarvis_storage::{CURRENT_CONFIG_VERSION, CURRENT_SCHEMA_VERSION};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BuildInfo {
    version: &'static str,
    target_os: &'static str,
    target_arch: &'static str,
    config_schema: u32,
    database_schema: i64,
}

impl BuildInfo {
    pub(crate) const fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            target_os: std::env::consts::OS,
            target_arch: std::env::consts::ARCH,
            config_schema: CURRENT_CONFIG_VERSION,
            database_schema: CURRENT_SCHEMA_VERSION,
        }
    }

    pub(crate) const fn version(self) -> &'static str {
        self.version
    }

    pub(crate) const fn target_os(self) -> &'static str {
        self.target_os
    }

    pub(crate) const fn target_arch(self) -> &'static str {
        self.target_arch
    }

    /// Returns the configuration schema version this build understands.
    pub(crate) const fn config_schema(self) -> u32 {
        self.config_schema
    }

    /// Returns the database schema version this build owns.
    pub(crate) const fn database_schema(self) -> i64 {
        self.database_schema
    }

    pub(crate) fn display_fields(self) -> String {
        format!(
            "version={} target={}-{} config_schema={} database_schema={}",
            self.version,
            self.target_arch,
            self.target_os,
            self.config_schema,
            self.database_schema
        )
    }

    pub(crate) fn display_line(self) -> String {
        format!("jarvisd {}", self.display_fields())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_metadata_contains_only_bounded_compile_time_identity() {
        let build = BuildInfo::current();
        let line = build.display_line();

        assert!(line.starts_with("jarvisd version="));
        // Derived from the constants so a schema bump cannot leave this test
        // asserting a version the build no longer reports.
        assert!(line.contains(&format!("config_schema={}", build.config_schema)));
        assert!(line.contains(&format!("database_schema={}", build.database_schema)));
        assert!(
            line.contains(&format!("database_schema={CURRENT_SCHEMA_VERSION}")),
            "the reported database schema must match the storage crate: {line}"
        );
        assert!(line.len() < 256);
        assert!(!line.contains('\n'));
        assert!(!line.contains('\r'));
    }
}
