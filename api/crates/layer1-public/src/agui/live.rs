use layer2_core::sandbox::{AgentLiveDelivery, AgentLiveFragmentKind};
use serde_json::json;

use super::events::AguiEvent;

pub(super) fn project(fragment: AgentLiveDelivery) -> AguiEvent {
    AguiEvent {
        kind: match fragment.kind {
            AgentLiveFragmentKind::Message => "TEXT_MESSAGE_CONTENT",
            AgentLiveFragmentKind::Thought => "REASONING_MESSAGE_CONTENT",
        },
        thread_id: fragment.agent_id.to_string(),
        run_id: fragment.turn_sequence.to_string(),
        body: json!({"messageId": fragment.message_id, "delta": fragment.delta}),
        timestamp: None,
        seq: None,
        live: true,
    }
}
