//! A tool whose **declaration** asks for an approval, shared by the daemon's test modules.
//!
//! # Why this is one module rather than a fixture in each test file
//!
//! Two tests need a call that policy **holds**: the inbound MCP route must refuse such a call (there is no
//! run to park), and the REST approval route must decide one. Each would otherwise build its own
//! definition, and the two copies would be two statements of one contract — the defect class this
//! repository has found repeatedly, where two values that must agree have nothing holding both. A fixture
//! that drifted would make one test exercise a tool the other does not, silently.
//!
//! # Why the declaration and not a workspace threshold
//!
//! The posture is `ApprovalPolicy::Ask` at **risk 0**. That matters: the workspace's default approval
//! threshold is `Moderate`, so a risk-0 tool cannot be held by the workspace at all. The hold therefore
//! comes from the tool's own declaration, which is a posture a real server can carry, and it holds
//! regardless of the workspace policy — so the tests do not depend on a policy default that could change.
//!
//! The field values are copied from `FilesystemReadTool`'s own definition rather than invented:
//! `EffectSet::single(ToolEffect::ReadOnly)` is what makes risk 0 legal, and `ToolSensitivity::new` is the
//! two-sided constructor rather than a bare enum, so every value here is one the contract accepts.

#![cfg(test)]

use jarvis_tools::ToolDefinition;

/// The identifier the fixture tool registers under.
///
/// A `jarvis.` namespace, so `ToolSource::from_namespace` classifies it `Native` — the fixture is code this
/// repository wrote, and `ToolDefinition::new` refuses a declared source that disagrees with the identifier.
pub const APPROVAL_TOOL: &str = "jarvis.test.approval";

const INPUT_SCHEMA: &str = r#"{
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": { "path": { "type": "string" } },
    "required": ["path"],
    "additionalProperties": false
}"#;

const OUTPUT_SCHEMA: &str = r#"{
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "type": "object",
    "properties": { "content": { "type": "string" } }
}"#;

/// A tool whose **declaration** asks for an approval its workspace would not.
///
/// # Panics
///
/// Panics only if a value this module hard-codes is rejected, which its own tests assert cannot happen. A
/// fallback here would be a posture nobody chose, which is the one outcome the definition type exists to
/// prevent.
#[must_use]
pub fn approval_declaring_definition() -> ToolDefinition {
    ToolDefinition::new(jarvis_tools::ToolDefinitionParts {
        id: jarvis_tools::ToolId::new(APPROVAL_TOOL)
            .unwrap_or_else(|error| panic!("{APPROVAL_TOOL} must be a valid identifier: {error}")),
        version: "1.0.0".to_owned(),
        title: "Approval-declaring test tool".to_owned(),
        description: "A fixture tool that declares an approval its workspace would not ask for."
            .to_owned(),
        input_schema: jarvis_tools::ToolSchema::parse(INPUT_SCHEMA)
            .unwrap_or_else(|error| panic!("the input schema must be accepted: {error}")),
        output_schema: jarvis_tools::ToolSchema::parse(OUTPUT_SCHEMA)
            .unwrap_or_else(|error| panic!("the output schema must be accepted: {error}")),
        effects: jarvis_tools::EffectSet::single(jarvis_tools::ToolEffect::ReadOnly),
        risk: 0,
        required_scopes: jarvis_tools::ScopeSet::none(),
        approval: jarvis_tools::ApprovalPolicy::Ask,
        timeout_seconds: 5,
        retry: jarvis_tools::RetryDeclaration::none(),
        idempotency: jarvis_tools::Idempotency::Required,
        source: jarvis_tools::ToolSource::Native,
        availability: jarvis_tools::Availability::Available,
        sensitivity: jarvis_tools::ToolSensitivity::new(
            jarvis_core::Sensitivity::Public,
            jarvis_core::Sensitivity::Public,
        ),
    })
    .unwrap_or_else(|error| panic!("the fixture definition must be accepted: {error}"))
}

/// An adapter that **counts** its calls, so "it did not run" is observed rather than inferred.
///
/// It returns a refusal rather than a confirmation, because a call that reaches it has already failed the
/// property under test — the count is what makes that visible, and a success value would let a test that
/// forgot to assert on the count pass.
#[derive(Default)]
pub struct ApprovalDeclaringAdapter {
    calls: std::sync::atomic::AtomicUsize,
}

impl ApprovalDeclaringAdapter {
    /// Returns how many times the adapter was reached.
    #[must_use]
    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl jarvis_tools::ToolExecutor for ApprovalDeclaringAdapter {
    fn adapter_id(&self) -> &'static str {
        "approval-fixture"
    }

    async fn execute(
        &self,
        _request: &jarvis_tools::ToolExecutionRequest,
    ) -> Result<jarvis_tools::ToolCallResult, jarvis_tools::AdapterError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(jarvis_tools::AdapterError::RefusedBeforeReaching {
            reason: "the approval fixture must never be reached".to_owned(),
        })
    }
}
