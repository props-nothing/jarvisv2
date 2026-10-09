//! The web fetch tool: a guarded, bounded HTTP `GET` whose body is returned as untrusted text.
//!
//! See `docs/research/integrations/web-fetch.md` for the sources behind each decision and
//! `docs/adr/0129-a-fetch-is-checked-on-the-address-it-connects-to.md` for the boundary.

mod address;
mod fetch;
mod search;
mod target;
mod text;

pub use address::is_public;
pub use fetch::{
    FETCH_SCOPE, FETCH_TOOL, MAX_BODY_BYTES, MAX_REDIRECTS, MAX_TEXT_CHARS, WebFetchTool,
    WebFetchToolError,
};
pub use search::{
    SEARCH_ENDPOINT, SEARCH_SCOPE, SEARCH_TOOL, WebSearchTool, WebSearchToolError,
    links_from_results,
};
pub use target::{EgressPolicy, MAX_URL_CHARS, Refusal, Target};
