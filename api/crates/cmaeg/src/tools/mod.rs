//! Shared authenticated tool operations used by REST and MCP.
mod context;
mod state;
pub use context::{Capabilities, ContextResponse, PublicActivity};
pub use state::ToolsState;
