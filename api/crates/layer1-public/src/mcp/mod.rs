//! MCP interface profile retained for the agent repoint in issue #817.

use crate::{InterfaceProfile, Operation};

pub struct McpInterface;

impl InterfaceProfile for McpInterface {
    fn interface_id(&self) -> &'static str {
        "mcp"
    }

    fn operations(&self) -> &'static [Operation] {
        &[]
    }
}
