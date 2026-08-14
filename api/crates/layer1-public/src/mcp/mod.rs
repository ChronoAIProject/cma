//! MCP interface (priority 4) — for agent clients.
//!
//! A projection of the Layer 2 core session model into MCP idiom.

use crate::{InterfaceProfile, Operation};

/// The MCP projection of the core session model.
#[derive(Debug, Default)]
pub struct McpInterface;

impl InterfaceProfile for McpInterface {
    fn interface_id(&self) -> &'static str {
        "mcp"
    }

    fn operations(&self) -> &'static [Operation] {
        &[
            Operation::CreateSession,
            Operation::SendInput,
            Operation::ResolveApproval,
        ]
    }
}
