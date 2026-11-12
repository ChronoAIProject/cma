use layer2_core::sandbox::{
    Agent, AgentLiveDelivery, AgentLiveFragmentKind, AgentLiveSubscription,
    MAX_DURABLE_AGENT_MESSAGE_BYTES,
};
use serde_json::json;
use tokio::sync::broadcast::error::TryRecvError;

use super::Open;

#[cfg(test)]
#[path = "stream_live_tests.rs"]
mod tests;

const MAX_LIVE_REASONING_BYTES: usize = 64 * 1024;

pub(super) struct LiveState {
    watching: Option<AgentLiveSubscription>,
    held: Option<AgentLiveDelivery>,
    pub(super) message_announced: bool,
    pub(super) message_index: usize,
    pub(super) message_text: String,
    reasoning_announced: bool,
    reasoning_index: usize,
    reasoning_text: String,
    reasoning_truncated: bool,
}

impl LiveState {
    pub(super) fn new(watching: Option<AgentLiveSubscription>) -> Self {
        Self {
            watching,
            held: None,
            message_announced: false,
            message_index: 0,
            message_text: String::new(),
            reasoning_announced: false,
            reasoning_index: 0,
            reasoning_text: String::new(),
            reasoning_truncated: false,
        }
    }
}

impl Open {
    pub(super) fn drain_live(&mut self, agent: &Agent, committed: u64) {
        let Some(mut watching) = self.live.watching.take() else {
            return;
        };
        for _ in 0..32 {
            let next = self
                .live
                .held
                .take()
                .map(Ok)
                .unwrap_or_else(|| watching.try_recv());
            let fragment = match next {
                Ok(fragment) => fragment,
                Err(TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            if fragment.agent_id != agent.id
                || Some(fragment.runtime_generation) != agent.conversation.runtime_generation
                || fragment.turn_sequence != self.turn
                || Some(self.turn) != agent.conversation.active_turn
            {
                continue;
            }
            if fragment.after_seq > agent.conversation.last_seq {
                self.live.held = Some(fragment);
                break;
            }
            match fragment.kind {
                AgentLiveFragmentKind::Message if fragment.after_seq >= committed => {
                    self.message_delta(&fragment.delta)
                }
                AgentLiveFragmentKind::Thought => self.thought_delta(&fragment.delta),
                AgentLiveFragmentKind::Message => {}
            }
        }
        self.live.watching = Some(watching);
    }

    fn message_delta(&mut self, delta: &str) {
        let delta = bounded_delta(
            delta,
            MAX_DURABLE_AGENT_MESSAGE_BYTES.saturating_sub(self.live.message_text.len()),
        );
        if delta.is_empty() {
            return;
        }
        let id = super::retrieve::output_id(&self.agent.id, self.turn, self.output_count);
        if !self.live.message_announced {
            self.live.message_index = self.next_output_index;
            self.next_output_index += 1;
            let index = self.live.message_index;
            self.push("response.output_item.added", json!({"output_index": index,
                "item": {"id": id, "type": "message", "status": "in_progress", "role": "assistant", "content": []}}));
            self.push("response.content_part.added", json!({"output_index": index, "item_id": id,
                "content_index": 0, "part": {"type": "output_text", "text": "", "annotations": [], "logprobs": []}}));
            self.live.message_announced = true;
        }
        let index = self.live.message_index;
        self.live.message_text.push_str(delta);
        self.push(
            "response.output_text.delta",
            json!({"output_index": index, "item_id": id,
            "content_index": 0, "delta": delta, "logprobs": []}),
        );
    }

    fn thought_delta(&mut self, delta: &str) {
        let bounded = bounded_delta(
            delta,
            MAX_LIVE_REASONING_BYTES.saturating_sub(self.live.reasoning_text.len()),
        );
        self.live.reasoning_truncated |= bounded.len() != delta.len();
        let delta = bounded;
        if delta.is_empty() {
            return;
        }
        let id = format!("rs_{}_{}", self.agent.id, self.turn);
        if !self.live.reasoning_announced {
            self.live.reasoning_index = self.next_output_index;
            self.next_output_index += 1;
            self.push("response.output_item.added", json!({"output_index": self.live.reasoning_index,
                "item": {"id": id, "type": "reasoning", "status": "in_progress", "summary": [], "cma:live": true}}));
            self.push(
                "response.reasoning_summary_part.added",
                json!({"item_id": id, "output_index": self.live.reasoning_index,
                "summary_index": 0, "part": {"type": "summary_text", "text": ""}}),
            );
            self.live.reasoning_announced = true;
        }
        self.live.reasoning_text.push_str(delta);
        self.push(
            "response.reasoning_summary_text.delta",
            json!({"item_id": id, "output_index": self.live.reasoning_index,
            "summary_index": 0, "delta": delta}),
        );
    }

    pub(super) fn finish_reasoning(&mut self) {
        if !self.live.reasoning_announced {
            return;
        }
        let text = std::mem::take(&mut self.live.reasoning_text);
        let id = format!("rs_{}_{}", self.agent.id, self.turn);
        self.push(
            "response.reasoning_summary_text.done",
            json!({"item_id": id, "output_index": self.live.reasoning_index,
            "summary_index": 0, "text": text, "cma:truncated": self.live.reasoning_truncated}),
        );
        self.push(
            "response.reasoning_summary_part.done",
            json!({"item_id": id, "output_index": self.live.reasoning_index,
            "summary_index": 0, "part": {"type": "summary_text", "text": text}}),
        );
        self.push(
            "response.output_item.done",
            json!({"output_index": self.live.reasoning_index,
            "item": {"id": id, "type": "reasoning", "status": "completed",
            "summary": [{"type": "summary_text", "text": text}], "cma:live": true}}),
        );
    }
}

fn bounded_delta(value: &str, limit: usize) -> &str {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
