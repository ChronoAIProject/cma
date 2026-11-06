use super::stream::{Following, Frame};
use axum::response::sse::Event;
use layer2_core::sandbox::{AgentLiveFragment, AgentLiveFragmentKind};
use serde_json::json;

impl Following {
    pub(super) fn queue_gap(&mut self, frames: Option<u64>, reason: &str) {
        let body = json!({"agent_id": self.header.id, "incarnation": self.identity,
            "turn_sequence": self.header.active_turn, "frames": frames, "reason": reason});
        self.pending.push_back(Frame {
            event: Event::default()
                .event("cma.agent.live_gap")
                .data(body.to_string()),
            live: true,
            fragment: None,
        });
    }

    pub(super) fn current_live_fragment(&self, fragment: &AgentLiveFragment) -> bool {
        fragment.agent_id == self.header.id
            && Some(fragment.runtime_generation) == self.header.runtime_generation
            && Some(fragment.turn_sequence) == self.header.active_turn
            && (fragment.kind == AgentLiveFragmentKind::Thought
                || fragment.after_seq >= self.committed_message)
    }

    pub(super) fn queue_live(&mut self) {
        let mut fragments: Vec<AgentLiveFragment> = Vec::new();
        for _ in 0..128 {
            let received = match self.held.take() {
                Some(fragment) => Ok(fragment),
                None => match &mut self.watching {
                    Some(watching) => watching.try_recv(),
                    None => break,
                },
            };
            match received {
                Ok(fragment) if self.current_live_fragment(&fragment) => {
                    if fragment.after_seq > self.header.head {
                        self.held = Some(fragment);
                        break;
                    }
                    if let Some(previous) = fragments.last_mut() {
                        if previous.message_id == fragment.message_id
                            && previous.kind == fragment.kind
                            && previous.turn_sequence == fragment.turn_sequence
                            && previous.runtime_generation == fragment.runtime_generation
                            && previous.after_seq == fragment.after_seq
                            && previous.delta.len() + fragment.delta.len() <= 16 * 1024
                        {
                            previous.delta.push_str(&fragment.delta);
                            previous.after_seq = fragment.after_seq;
                            continue;
                        }
                    }
                    fragments.push(fragment);
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(frames)) => {
                    self.queue_fragments(&mut fragments);
                    self.queue_gap(Some(frames), "lagged");
                }
                Err(_) => break,
            }
        }
        self.queue_fragments(&mut fragments);
    }

    fn queue_fragments(&mut self, fragments: &mut Vec<AgentLiveFragment>) {
        self.live_batch_needs_validation |= !fragments.is_empty();
        for fragment in fragments.drain(..) {
            let mut body = json!({"agent_id": fragment.agent_id, "incarnation": self.identity,
                "turn_sequence": fragment.turn_sequence, "after_sequence": fragment.after_seq,
                "message_id": fragment.message_id, "kind": match fragment.kind {
                    AgentLiveFragmentKind::Message => "text_delta",
                    AgentLiveFragmentKind::Thought => "reasoning_summary_delta",
                }, "delta": fragment.delta});
            // Negotiate this field: existing strict consumers must keep their old shape.
            // This is this server's live observation time, not a provider generation time.
            if self.live_timestamps {
                body["observed_at"] = json!(layer2_core::now_rfc3339());
            }
            self.pending.push_back(Frame {
                event: Event::default()
                    .event("cma.agent.live")
                    .data(body.to_string()),
                live: true,
                fragment: Some(fragment),
            });
        }
    }
}
