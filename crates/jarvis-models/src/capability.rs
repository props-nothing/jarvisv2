use serde::{Deserialize, Serialize};

/// Whether a capability is known to be available.
///
/// This is deliberately three-valued. A two-valued flag would force an unknown
/// capability to be reported as either supported or unsupported, and reporting an
/// unprobed capability as supported is how a feature silently fails open.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    /// The capability is known to be available.
    Supported,
    /// The capability is known to be unavailable.
    Unsupported,
    /// The capability has not been established.
    #[default]
    Unknown,
}

impl Support {
    /// Returns whether the capability is confirmed available.
    ///
    /// Callers must use this, not `!is_unsupported()`, so that an unprobed
    /// capability cannot enable a code path.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Supported)
    }

    /// Returns whether the capability is confirmed unavailable.
    #[must_use]
    pub const fn is_unsupported(self) -> bool {
        matches!(self, Self::Unsupported)
    }

    /// Returns the stable snake-case wire code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
        }
    }
}

impl From<bool> for Support {
    fn from(value: bool) -> Self {
        if value {
            Self::Supported
        } else {
            Self::Unsupported
        }
    }
}

/// Where a model executes, which is a privacy input rather than a performance hint.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// The model runs on this machine and the prompt never leaves it.
    Local,
    /// The prompt is transmitted to a third party.
    Remote,
    /// The placement has not been established.
    ///
    /// Treated as remote by policy, because assuming local would permit private
    /// content to leave the machine on an unverified assumption.
    Unknown,
}

impl Placement {
    /// Returns the stable snake-case wire code.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Unknown => "unknown",
        }
    }
}

/// What a model and its provider are known to support.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelCapabilities {
    context_window_tokens: Option<u32>,
    max_output_tokens: Option<u32>,
    streaming: Support,
    tool_calls: Support,
    structured_output: Support,
    image_input: Support,
    usage_on_stream: Support,
    developer_role: Support,
    placement: Option<Placement>,
}

impl ModelCapabilities {
    /// Creates a capability set in which every feature is unprobed.
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            context_window_tokens: None,
            max_output_tokens: None,
            streaming: Support::Unknown,
            tool_calls: Support::Unknown,
            structured_output: Support::Unknown,
            image_input: Support::Unknown,
            usage_on_stream: Support::Unknown,
            developer_role: Support::Unknown,
            placement: None,
        }
    }

    /// Sets the maximum prompt and completion tokens the model accepts.
    #[must_use]
    pub const fn with_context_window_tokens(mut self, value: Option<u32>) -> Self {
        self.context_window_tokens = value;
        self
    }

    /// Sets the maximum completion size.
    #[must_use]
    pub const fn with_max_output_tokens(mut self, value: Option<u32>) -> Self {
        self.max_output_tokens = value;
        self
    }

    /// Sets streaming support.
    #[must_use]
    pub const fn with_streaming(mut self, value: Support) -> Self {
        self.streaming = value;
        self
    }

    /// Sets tool-call support.
    #[must_use]
    pub const fn with_tool_calls(mut self, value: Support) -> Self {
        self.tool_calls = value;
        self
    }

    /// Sets structured-output support.
    #[must_use]
    pub const fn with_structured_output(mut self, value: Support) -> Self {
        self.structured_output = value;
        self
    }

    /// Sets image-input support.
    #[must_use]
    pub const fn with_image_input(mut self, value: Support) -> Self {
        self.image_input = value;
        self
    }

    /// Sets whether token usage is reported on the streaming path.
    #[must_use]
    pub const fn with_usage_on_stream(mut self, value: Support) -> Self {
        self.usage_on_stream = value;
        self
    }

    /// Sets whether the server accepts the higher-authority instruction role.
    ///
    /// `OpenAI` documents `developer` as outranking `user`, while several compatible
    /// servers accept only `system` for the same purpose. Recording this as a
    /// capability keeps the difference explicit instead of hard-coding one spelling.
    #[must_use]
    pub const fn with_developer_role(mut self, value: Support) -> Self {
        self.developer_role = value;
        self
    }

    /// Sets where the model executes.
    #[must_use]
    pub const fn with_placement(mut self, value: Option<Placement>) -> Self {
        self.placement = value;
        self
    }

    /// Returns the maximum prompt and completion tokens, when known.
    #[must_use]
    pub const fn context_window_tokens(&self) -> Option<u32> {
        self.context_window_tokens
    }

    /// Returns the maximum completion size, when known.
    #[must_use]
    pub const fn max_output_tokens(&self) -> Option<u32> {
        self.max_output_tokens
    }

    /// Returns streaming support.
    #[must_use]
    pub const fn streaming(&self) -> Support {
        self.streaming
    }

    /// Returns tool-call support.
    #[must_use]
    pub const fn tool_calls(&self) -> Support {
        self.tool_calls
    }

    /// Returns structured-output support.
    #[must_use]
    pub const fn structured_output(&self) -> Support {
        self.structured_output
    }

    /// Returns image-input support.
    #[must_use]
    pub const fn image_input(&self) -> Support {
        self.image_input
    }

    /// Returns whether usage is reported on the streaming path.
    #[must_use]
    pub const fn usage_on_stream(&self) -> Support {
        self.usage_on_stream
    }

    /// Returns whether the server accepts the higher-authority instruction role.
    #[must_use]
    pub const fn developer_role(&self) -> Support {
        self.developer_role
    }

    /// Returns where the model executes.
    ///
    /// A missing placement reports [`Placement::Unknown`] rather than local, so
    /// policy that gates private content on locality fails closed.
    #[must_use]
    pub const fn placement(&self) -> Placement {
        match self.placement {
            Some(value) => value,
            None => Placement::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unprobed_capability_does_not_report_as_supported() {
        let capabilities = ModelCapabilities::unknown();
        assert_eq!(capabilities.streaming(), Support::Unknown);
        assert!(
            !capabilities.streaming().is_supported(),
            "an unprobed capability must not enable a code path"
        );
        assert!(!capabilities.streaming().is_unsupported());
    }

    #[test]
    fn an_unprobed_placement_is_not_assumed_local() {
        assert_eq!(
            ModelCapabilities::unknown().placement(),
            Placement::Unknown,
            "assuming local would let private content leave the machine unverified"
        );
    }

    #[test]
    fn support_values_round_trip_through_the_wire_form() {
        for value in [Support::Supported, Support::Unsupported, Support::Unknown] {
            let json = serde_json::to_string(&value).unwrap_or_default();
            let decoded: Result<Support, _> = serde_json::from_str(&json);
            assert_eq!(decoded.ok(), Some(value));
            assert!(json.contains(value.as_str()));
        }
        assert_eq!(Support::from(true), Support::Supported);
        assert_eq!(Support::from(false), Support::Unsupported);
    }
}
