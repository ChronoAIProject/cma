use serde::{Deserialize, Serialize};

use crate::{RuntimeCapabilities, RuntimeCapability};

use super::AGENT_IO_SCHEMA_VERSION;

/// A projection of demonstrated core operations, further constrained by each
/// public interface. Busy messages use the durable command queue; live deltas
/// remain distinct from replayable committed message events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentInterfaceCapabilities {
    pub schema_versions: Vec<u16>,
    pub text_input: bool,
    pub attachment_input: bool,
    pub tool_events: bool,
    pub approval_control: bool,
    pub steer: bool,
    pub interrupt: bool,
    pub durable_event_replay: bool,
    pub durable_text_deltas: bool,
    pub busy_message_queue: bool,
}

impl AgentInterfaceCapabilities {
    pub fn from_runtime(runtime: &RuntimeCapabilities) -> Self {
        let conversation = runtime.supports(RuntimeCapability::Conversation);
        Self {
            schema_versions: vec![AGENT_IO_SCHEMA_VERSION],
            text_input: conversation,
            attachment_input: false,
            tool_events: conversation,
            approval_control: conversation,
            steer: conversation && runtime.supports(RuntimeCapability::Steer),
            interrupt: conversation && runtime.supports(RuntimeCapability::Interrupt),
            durable_event_replay: true,
            durable_text_deltas: false,
            busy_message_queue: conversation,
        }
    }
}
