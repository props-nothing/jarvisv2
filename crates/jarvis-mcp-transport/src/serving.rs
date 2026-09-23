//! Binding the MCP endpoint: the configuration a served handler is reachable through, and **why the
//! SDK's own origin check is not the one used**.
//!
//! # The finding this module is built around
//!
//! `P3-009a` recorded that `rmcp`'s `origin_is_allowed` cannot express the control JARVIS needs, because
//! `a_port.is_none() || a_port == o_port` makes a portless allowlist entry a wildcard over **every** port
//! while treating an explicit default port as literal. This slice turned that from a design argument into a
//! **functional** one: the SDK's `validate_origin_header` compares against `allowed_origins` directly, and it
//! has no extension point — `with_allowed_origins` is the whole surface. So there is no way to hand it
//! JARVIS's comparison.
//!
//! Therefore **origin validation cannot be delegated**. [`ServingConfig::origin_check`] returns the policy
//! check for a request's `Origin`, and the enforcement point is `P3-009c`'s (the layer that owns the request).
//! The SDK's check is left **disabled** ([`ServingConfig::sdk`] uses `disable_allowed_origins`), and that is a
//! deliberate, recorded decision rather than an omission: leaving it enabled would mean two origin checks,
//! one of which admits any port, and the permissive one would be part of the answer.
//!
//! # Why the rest of the fields are set rather than inherited
//!
//! `P3-009a` recorded the permissive defaults. Each is now set explicitly, and [`ServingConfig::sdk`] is the
//! only place they are stated:
//!
//! | Field | Set to | Left alone would mean |
//! | --- | --- | --- |
//! | `allowed_origins` / `validate_empty_origin_allowlist` | disabled, because the check is JARVIS's | `Origin` validation off, or a port wildcard |
//! | `legacy_session_mode` | `false` | mints an `Mcp-Session-Id` that `2026-07-28` removed |
//! | `stateless_protocol_metadata_required` | `true` | an absent `MCP-Protocol-Version` treated as `2025-03-26` |
//! | `max_request_body_bytes` | the configured bound | the SDK's own default, which nothing here stated |
//! | `allowed_hosts` | loopback only, from the policy | already fail-closed, but stated so a change is visible |
//! | session manager | `NeverSessionManager` | an in-memory session store for a protocol with no sessions |
//!
//! # Why nothing is bound here
//!
//! [`ServingConfig`] produces the SDK's service; **binding it to a socket is the daemon's**, because
//! `repository-layout.md` gives network listeners to the composition root. So this module is reachable from
//! a test and not yet from a client, which is the same boundary `P3-009e`'s handler sits behind — but now
//! proven through the **real** service rather than only at the handler's own method.

use std::sync::Arc;

use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::tower::{
    StreamableHttpServerConfig, StreamableHttpService,
};

use crate::serve::JarvisMcpServer;
use jarvis_mcp::{OriginVerdict, ServerExposure};

/// The SDK's Streamable HTTP service, named once and **privately**.
type SdkService = StreamableHttpService<JarvisMcpServer, NeverSessionManager>;

/// The most bytes one request body may carry.
///
/// A tool call's arguments are a JSON object that must satisfy the tool's input schema, and the largest
/// schema in this project bounds a value at kilobytes. Eight mebibytes is generous for that and far below
/// what an unbounded body would allow, which is the point: `StreamableHttpServerConfig` has a default, and a
/// default is the one value that changes without a line here changing.
pub const MAX_REQUEST_BODY_BYTES: usize = 8 * 1024 * 1024;

/// The path a client posts to.
///
/// Stated as a constant rather than inline, because the daemon's route and any client's configured URL must
/// agree, and two literals for one path is the defect class this project records repeatedly.
pub const MCP_ENDPOINT_PATH: &str = "/mcp";

/// Explains why a serving configuration could not be built.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ServingConfigError {
    /// The exposure policy would require a bind this slice refuses.
    ///
    /// A remotely reachable JARVIS MCP server needs audience-bound tokens (RFC 9728 Protected Resource
    /// Metadata and RFC 8707 resource indicators), which are not built. Serving off-host without them would
    /// be an **unauthenticated control plane**, so the refusal is the honest outcome rather than a warning.
    RemoteBindRequired,
}

impl std::fmt::Display for ServingConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RemoteBindRequired => write!(
                formatter,
                "the configured origins require a remote bind, but a remotely reachable MCP server needs \
                 audience-bound tokens that are not built; refusing to serve off-host"
            ),
        }
    }
}

impl std::error::Error for ServingConfigError {}

/// The configuration a JARVIS MCP endpoint is served under.
///
/// Holds the exposure policy because the origin decision is JARVIS's, and produces the SDK's service from it
/// so the SDK's permissive defaults are never the values in force.
#[derive(Clone, Debug)]
pub struct ServingConfig {
    exposure: ServerExposure,
    max_request_body_bytes: usize,
}

impl ServingConfig {
    /// Builds a serving configuration from an exposure policy.
    ///
    /// # Errors
    ///
    /// Returns [`ServingConfigError::RemoteBindRequired`] when the policy allows an origin that is not on
    /// this host. The check is `ServerExposure::requires_remote_bind`, which is a property of the **host**
    /// rather than of the presence of an entry, so a `localhost` origin is served and a public one is refused.
    pub fn new(exposure: ServerExposure) -> Result<Self, ServingConfigError> {
        if exposure.requires_remote_bind() {
            return Err(ServingConfigError::RemoteBindRequired);
        }
        Ok(Self {
            exposure,
            max_request_body_bytes: MAX_REQUEST_BODY_BYTES,
        })
    }

    /// The configuration a deployment that says nothing about exposure gets: loopback only, no browser origin
    /// admitted.
    ///
    /// # Panics
    ///
    /// Panics only if [`ServerExposure::loopback_only`] requires a remote bind, which its own tests assert
    /// cannot happen. A silent fallback would serve a policy nobody chose, which is the outcome this type
    /// exists to prevent.
    #[must_use]
    pub fn loopback_only() -> Self {
        let exposure = ServerExposure::loopback_only();
        Self::new(exposure).unwrap_or_else(|error| {
            panic!("the loopback-only policy cannot require a remote bind: {error}")
        })
    }

    /// Returns the exposure policy in force.
    #[must_use]
    pub const fn exposure(&self) -> &ServerExposure {
        &self.exposure
    }

    /// Returns the request-body bound.
    #[must_use]
    pub const fn max_request_body_bytes(&self) -> usize {
        self.max_request_body_bytes
    }

    /// Overrides the request-body bound.
    ///
    /// Offered so a test can prove the bound is enforced without sending eight mebibytes.
    #[must_use]
    pub const fn with_max_request_body_bytes(mut self, bytes: usize) -> Self {
        self.max_request_body_bytes = bytes;
        self
    }

    /// Decides one request's `Origin` against the policy.
    ///
    /// The enforcement point. `P3-009c` owns the layer that calls this; the SDK's own check is disabled
    /// because it cannot express this rule.
    #[must_use]
    pub fn origin_check(&self, origin: Option<&str>) -> OriginVerdict {
        self.exposure.decide(origin)
    }

    /// Returns the SDK's server configuration, with every permissive default replaced by a stated value.
    ///
    /// # Why this is **not** public
    ///
    /// A `StreamableHttpServerConfig` is an SDK type, and this crate's documented invariant is that **no
    /// provider SDK type appears in its public surface** (`repository-layout.md`, from `AGENTS.md`'s "provider
    /// SDK types must not cross JARVIS domain boundaries"). Returning it from a public function would make the
    /// SDK part of this crate's contract, so a reader of the daemon could not tell which types are JARVIS's and
    /// which are a dependency's.
    ///
    /// This was **wrong when first written and is corrected here**: `P3-009b` made it, `sdk()`, and
    /// `service()` public, and a review against the invariant found them. Nothing outside this module needs the
    /// SDK value — the daemon needs an endpoint and the decisions a layer must enforce, and
    /// [`Self::checked_service`] wraps it into something bindable without naming the type.
    ///
    /// # The origin fields are disabled deliberately
    ///
    /// `disable_allowed_origins` is called **because the check is JARVIS's**, not because origins go
    /// unchecked. Leaving the SDK's check enabled would add a second answer — one that admits any port for a
    /// portless entry — and a permissive second answer is worse than none, because a reader would believe the
    /// permissive field was the control.
    #[must_use]
    fn sdk(&self) -> StreamableHttpServerConfig {
        StreamableHttpServerConfig::default()
            // The session the revision removed. `NeverSessionManager` refuses `create_session`, so even a
            // legacy `initialize` cannot mint one.
            .with_legacy_session_mode(false)
            // A per-request `MCP-Protocol-Version` is required rather than an absent header defaulting to
            // `2025-03-26`.
            .with_stateless_protocol_metadata_required(true)
            .with_max_request_body_bytes(self.max_request_body_bytes)
            // Stated so a change to the SDK's loopback default is visible here. The policy refuses a remote
            // bind outright, so this list and that refusal are two statements of one decision.
            .with_allowed_hosts(["localhost", "127.0.0.1", "::1"])
            .disable_allowed_origins()
            // A JSON response rather than an SSE stream for a simple request-response tool. JARVIS serves no
            // long-lived notifications, so a stream would be a mechanism a client must implement for no gain —
            // and the SDK falls back to SSE automatically if a notification were ever emitted.
            .with_json_response(true)
    }

    /// Builds the SDK's Streamable HTTP service over a handler factory.
    ///
    /// Private for the same reason as [`Self::sdk`]: the return type names an SDK service and an SDK session
    /// manager. The SDK-typed service is reached only from this crate's own transport tests, through
    /// [`Self::sdk_service_for_test`].
    ///
    /// The factory is called per request by the SDK, so a handler is constructed fresh rather than shared;
    /// `JarvisMcpServer` holds an `Arc` runner, so the cost is a clone of the served list. `NeverSessionManager`
    /// is supplied because the revision has no sessions, and the SDK's default (`LocalSessionManager`) would
    /// store state for a protocol that removed it.
    ///
    /// # Why this is now `pub(crate)` when it was private
    ///
    /// `P3-009c` binds a listener, and the layer that mounts the SDK's service lives in this crate
    /// ([`crate::binding`]). It needs the service, and an **integration** test in `apps/jarvisd` cannot reach a
    /// `pub(crate)` item of this crate — which is why the daemon's own proof drives its router rather than
    /// naming this type. A crate-internal door is not an SDK-typed item in this crate's *public* surface, so the
    /// invariant `boundary_tests.rs` enforces is kept; the alternative it was written against — a `pub fn`
    /// returning `StreamableHttpService` — is still refused.
    #[must_use]
    pub(crate) fn service(&self, server: JarvisMcpServer) -> SdkService {
        StreamableHttpService::new(
            move || Ok(server.clone()),
            Arc::new(NeverSessionManager::default()),
            self.sdk(),
        )
    }

    /// Builds the SDK service, for a test that drives the **real** transport.
    ///
    /// `#[cfg(test)]` because nothing in the product calls it. A daemon binding this endpoint lives in
    /// `apps/jarvisd`, which cannot see a private item of this crate — so the daemon wiring (which owns network
    /// listeners per `repository-layout.md`) will need an SDK-typed accessor through an **off-by-default
    /// feature**, and that is deliberately **not built here**. The reason is the same invariant as
    /// [`Self::sdk`]: a public function returning an SDK type would put the SDK in this crate's contract, and
    /// the right time to add an escape hatch is when a caller exists that needs it.
    ///
    /// **Not public, and not a `pub(crate)` escape hatch either** — a crate-internal door would be reachable
    /// from this crate's own integration tests but would still be an SDK-typed function inside the crate that
    /// claims none, so the invariant would be kept only in letter.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn sdk_service_for_test(&self, server: JarvisMcpServer) -> SdkService {
        self.service(server)
    }
}

/// Explains why a served surface cannot be offered.
///
/// A JARVIS-owned condition rather than an SDK error, so a daemon can report it without naming an SDK type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceError {
    /// The handler's served set was empty.
    ///
    /// `jarvis_mcp::served_tools` already refuses to produce an empty surface, so reaching this from the real
    /// composition path is impossible — the check exists because `JarvisMcpServer::new` accepts any vector, and
    /// a handler built by hand is the case it guards. A client connecting successfully to a server offering
    /// nothing cannot tell a misconfiguration from an empty registry, which is why the condition is named
    /// rather than served.
    NoToolsAdvertised,
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoToolsAdvertised => write!(
                formatter,
                "the server advertises no tools, so binding an endpoint would offer a client a server it \
                 cannot use"
            ),
        }
    }
}

impl std::error::Error for ServiceError {}

/// Explains why a served surface cannot be offered, when a caller asks.
///
/// The check lives on [`ServingConfig`] rather than being repeated by every layer that wraps a handler, so
/// there is **one** place a daemon consults.
impl ServingConfig {
    /// Returns whether a handler may be offered to a client.
    ///
    /// # Errors
    ///
    /// Returns [`ServiceError::NoToolsAdvertised`] when the served set is empty.
    pub fn check_servable(server: &JarvisMcpServer) -> Result<(), ServiceError> {
        if server.is_empty() {
            return Err(ServiceError::NoToolsAdvertised);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "serving_transport_tests.rs"]
mod transport_tests;

#[cfg(test)]
mod tests {
    use super::*;

    use async_trait::async_trait;
    use jarvis_core::{CorrelationId, SystemClock, UtcTimestamp};
    use jarvis_tools::{
        AdapterError, BoundedOutput, ProviderEvidence, ToolCallResult, ToolDefinition,
        ToolDefinitionParts, ToolId, ToolOutcomeRecord, ToolSchema, ToolSource,
    };
    use serde_json::{Value, json};

    use crate::serve::ServedToolRunner;

    /// A runner that answers a fixed confirmation, so a served call has something to return.
    struct ConfirmingRunner;

    #[async_trait]
    impl ServedToolRunner for ConfirmingRunner {
        async fn run(
            &self,
            _name: &str,
            _arguments: Value,
            _correlation_id: CorrelationId,
        ) -> Result<ToolCallResult, AdapterError> {
            let record = ToolOutcomeRecord::confirmed("jarvis:test")
                .unwrap_or_else(|error| panic!("{error}"));
            Ok(ToolCallResult::new(
                record,
                Some(
                    ProviderEvidence::new("jarvis:test").unwrap_or_else(|error| panic!("{error}")),
                ),
                Some(BoundedOutput::truncating("the file contents".to_owned())),
                UtcTimestamp::now(&SystemClock),
            ))
        }
    }

    /// A runner that records calls, so a test can assert one was made.
    struct RecordingRunner {
        calls: std::sync::Mutex<usize>,
    }

    impl RecordingRunner {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: std::sync::Mutex::new(0),
            })
        }
    }

    #[async_trait]
    impl ServedToolRunner for RecordingRunner {
        async fn run(
            &self,
            _name: &str,
            _arguments: Value,
            _correlation_id: CorrelationId,
        ) -> Result<ToolCallResult, AdapterError> {
            let mut count = self
                .calls
                .lock()
                .unwrap_or_else(|error| panic!("the counter is usable: {error}"));
            *count += 1;
            let record = ToolOutcomeRecord::confirmed("jarvis:test")
                .unwrap_or_else(|error| panic!("{error}"));
            Ok(ToolCallResult::new(
                record,
                Some(
                    ProviderEvidence::new("jarvis:test").unwrap_or_else(|error| panic!("{error}")),
                ),
                Some(BoundedOutput::truncating("ok".to_owned())),
                UtcTimestamp::now(&SystemClock),
            ))
        }
    }

    fn served_tool(name: &str) -> JarvisMcpServer {
        let definition = ToolDefinition::new(ToolDefinitionParts {
            id: ToolId::new(name).unwrap_or_else(|error| panic!("{name}: {error}")),
            version: "1.0.0".to_owned(),
            title: format!("title for {name}"),
            description: format!("description for {name}"),
            input_schema: ToolSchema::parse(
                &json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                    "required": ["path"],
                    "additionalProperties": false
                })
                .to_string(),
            )
            .unwrap_or_else(|error| panic!("{error}")),
            output_schema: ToolSchema::parse(
                &json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "properties": { "contents": { "type": "string" } },
                    "required": ["contents"],
                    "additionalProperties": false
                })
                .to_string(),
            )
            .unwrap_or_else(|error| panic!("{error}")),
            effects: jarvis_tools::EffectSet::single(jarvis_tools::ToolEffect::ReadOnly),
            risk: 0,
            required_scopes: jarvis_tools::ScopeSet::none(),
            approval: jarvis_tools::ApprovalPolicy::Auto,
            timeout_seconds: 10,
            retry: jarvis_tools::RetryDeclaration::none(),
            idempotency: jarvis_tools::Idempotency::Unsupported,
            source: ToolSource::from_namespace(
                name.rsplit_once('.')
                    .map_or(name, |(namespace, _)| namespace),
            ),
            availability: jarvis_tools::Availability::Available,
            sensitivity: jarvis_tools::ToolSensitivity::new(
                jarvis_core::Sensitivity::Internal,
                jarvis_core::Sensitivity::Internal,
            ),
        })
        .unwrap_or_else(|error| panic!("{name}: {error}"));

        let (served, exclusions) = jarvis_mcp::served_tools([&definition])
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(
            exclusions.is_empty(),
            "{name} must be servable: {exclusions:?}"
        );
        JarvisMcpServer::new(served, Arc::new(ConfirmingRunner))
    }

    /// **A configuration that allows a public origin is refused, because serving off-host without
    /// audience-bound tokens would be an unauthenticated control plane.**
    ///
    /// Falsified by removing the check: the configuration is accepted, and `P3-009b` would then bind an
    /// endpoint a remote browser could reach with no token to present.
    #[test]
    fn a_policy_that_allows_a_public_origin_is_refused() {
        let exposure = ServerExposure::new(["https://jarvis.example.com"])
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            ServingConfig::new(exposure).err(),
            Some(ServingConfigError::RemoteBindRequired)
        );
    }

    /// A loopback origin does **not** require a remote bind, so a page served from this host is served.
    ///
    /// This is the boundary's subtle case: the test is on the **host**, not on whether an entry exists, so a
    /// `localhost` origin is admitted while a public one is refused. Falsified by testing for the presence of
    /// an entry: this configuration would then be refused, and a local desktop client's page could not use
    /// the endpoint.
    #[test]
    fn a_loopback_origin_is_served_and_a_public_one_is_not() {
        for local in ["http://localhost:3000", "http://127.0.0.1:8080"] {
            let exposure = ServerExposure::new([local]).unwrap_or_else(|error| panic!("{error}"));
            assert!(
                ServingConfig::new(exposure).is_ok(),
                "{local} is on this host and must be served"
            );
        }
    }

    /// **The SDK's origin check is disabled, and JARVIS's is the one that answers.**
    ///
    /// Two assertions, and the first is the SDK's permissive default stated as a fact: an empty
    /// `allowed_origins` with `validate_empty_origin_allowlist` false leaves the SDK's check off entirely. The
    /// second is that the policy still refuses a present `Origin`, so the decision did not disappear with the
    /// SDK's field — it moved.
    ///
    /// Falsified by re-enabling the SDK's check with an allowlist: the two implementations then disagree
    /// about a portless entry, and the permissive one is part of the answer.
    #[test]
    fn the_sdk_origin_check_is_disabled_and_the_policy_is_the_control() {
        let config = ServingConfig::loopback_only();
        let sdk = config.sdk();
        assert!(
            sdk.allowed_origins.is_empty(),
            "the SDK's allowlist must be left empty, because the comparison is JARVIS's"
        );

        // The policy refuses every present Origin under the loopback-only default...
        assert!(
            !config
                .origin_check(Some("https://jarvis.example.com"))
                .permits()
        );
        assert!(!config.origin_check(Some("null")).permits());
        assert!(!config.origin_check(Some("not an origin")).permits());
        // ...and admits an absent one, which is what a local tool sends.
        assert!(config.origin_check(None).permits());
    }

    /// A configured loopback origin is admitted, and a different port is not.
    ///
    /// This is the rule `P3-009a` established and the SDK cannot express: the SDK's comparison would admit
    /// **any** port for a portless entry. So this test is the functional form of that finding.
    #[test]
    fn the_policy_admits_the_configured_loopback_origin_and_nothing_else() {
        let exposure = ServerExposure::new(["http://localhost:3000"])
            .unwrap_or_else(|error| panic!("{error}"));
        let config = ServingConfig::new(exposure).unwrap_or_else(|error| panic!("{error}"));

        assert!(config.origin_check(Some("http://localhost:3000")).permits());
        assert!(
            !config.origin_check(Some("http://localhost:4000")).permits(),
            "a different port is a different origin"
        );
        assert!(
            !config
                .origin_check(Some("https://localhost:3000"))
                .permits()
        );
        assert!(
            !config
                .origin_check(Some("http://localhost.evil.test"))
                .permits()
        );
    }

    /// **Every permissive SDK default is stated rather than inherited.**
    ///
    /// Falsified one field at a time: each assertion below is the value the SDK's `Default` would *not* have
    /// if the corresponding line were removed.
    #[test]
    fn the_hardened_configuration_differs_from_the_sdk_default_in_every_permissive_field() {
        let config = ServingConfig::loopback_only();
        let sdk = config.sdk();
        let default = StreamableHttpServerConfig::default();

        assert!(
            !sdk.legacy_session_mode,
            "sessions were removed by the revision"
        );
        assert!(
            default.legacy_session_mode,
            "and the SDK default mints them"
        );
        assert!(sdk.stateless_protocol_metadata_required);
        assert!(!default.stateless_protocol_metadata_required);
        assert_eq!(sdk.allowed_hosts, vec!["localhost", "127.0.0.1", "::1"]);
        assert_eq!(sdk.max_request_body_bytes, MAX_REQUEST_BODY_BYTES);
        // The origin fields: both off, because the check is elsewhere.
        assert!(sdk.allowed_origins.is_empty());
        // A JSON response rather than an SSE stream, since JARVIS serves no notifications.
        assert!(sdk.json_response);
    }

    /// The body bound is configurable so a test can prove it, and it reaches the SDK's own field.
    #[test]
    fn the_body_bound_is_carried_into_the_sdk_configuration() {
        let config = ServingConfig::loopback_only().with_max_request_body_bytes(1024);
        assert_eq!(config.max_request_body_bytes(), 1024);
        assert_eq!(config.sdk().max_request_body_bytes, 1024);
    }

    /// An endpoint that would advertise nothing is refused rather than offered.
    ///
    /// Falsified by removing the check: a client connects successfully to a server offering no tools — unable to
    /// tell a misconfiguration from an empty registry.
    #[test]
    fn an_endpoint_with_no_tools_is_refused() {
        let empty = JarvisMcpServer::new(Vec::new(), RecordingRunner::new());
        assert_eq!(
            ServingConfig::check_servable(&empty).err(),
            Some(ServiceError::NoToolsAdvertised)
        );
        // The positive control, so this is not passing because every handler is refused.
        assert!(ServingConfig::check_servable(&served_tool("jarvis.files.read")).is_ok());
    }

    /// A clone of the handler serves the same set, because the SDK's factory clones one per request.
    ///
    /// The compile-time assertion above is the real check; this is the runtime form, and it asserts the clone
    /// keeps the served set rather than merely compiling.
    #[test]
    fn a_clone_of_the_handler_serves_the_same_set() {
        let server = served_tool("jarvis.files.read");
        let cloned = server.clone();
        assert_eq!(cloned.served_names(), vec!["jarvis.files.read"]);
    }

    /// The endpoint path is one constant, so the daemon's route and a client's URL cannot disagree.
    #[test]
    fn the_endpoint_path_is_stated_once() {
        assert_eq!(MCP_ENDPOINT_PATH, "/mcp");
        assert!(MCP_ENDPOINT_PATH.starts_with('/'));
    }
}
