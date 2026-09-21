//! Provider-neutral model gateway contracts.
//!
//! A **provider** authenticates and transports a request; a **model** is the
//! selectable capability. `crates/jarvis-core` owns runs, approvals, and
//! persistence, and no provider SDK type crosses that boundary.
//!
//! # Normalized Errors
//!
//! Provider failures are converted into [`ModelError`], whose kind maps onto the
//! existing [`jarvis_core::ErrorCode`] categories. The mapping is not a rename:
//! provider quota and billing failures are `PermanentUpstream`, because retrying
//! them never restores access.
//!
//! # Streaming
//!
//! A stream is a `Stream` of [`StreamEnvelope`] values carrying a contiguous
//! sequence number. [`StreamValidator`] enforces the ordering rules and reports a
//! gap, a late event, or a missing finish, so a truncated answer is never mistaken
//! for a complete one.
//!
//! ```
//! use jarvis_models::{FinishReason, ModelId, ProviderId, Role, ChatMessage, ChatRequest};
//!
//! let provider = ProviderId::new("openai-compatible")?;
//! let model = ModelId::new("gpt-oss:20b")?;
//! let request = ChatRequest::new(
//!     model,
//!     vec![ChatMessage::user("Say this is a test")],
//!     jarvis_core::CorrelationId::new(),
//! );
//!
//! assert_eq!(provider.as_str(), "openai-compatible");
//! assert_eq!(request.messages().len(), 1);
//! assert_eq!(FinishReason::from_provider_label("stop"), FinishReason::Stop);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod capability;
mod error;
mod identity;
mod port;
mod request;
mod response;
mod stream;
mod usage;

pub mod openai;

pub use capability::{ModelCapabilities, Placement, Support};
pub use error::{ModelError, ModelErrorKind, ProviderRequestId};
pub use identity::{IdentityError, MAX_IDENTIFIER_BYTES, ModelId, ProviderId};
pub use port::{ModelGateway, ModelStream, ProviderHealth, ProviderStatus};
pub use request::{
    ChatMessage, ChatRequest, ContentPart, MAX_MESSAGE_BYTES, MAX_MESSAGES, MessageContent, Role,
};
pub use response::{ChatResponse, FinishReason, OutputContent, ToolCall};
pub use stream::{StreamEnvelope, StreamEvent, StreamEventError, StreamSummary, StreamValidator};
pub use usage::TokenUsage;
