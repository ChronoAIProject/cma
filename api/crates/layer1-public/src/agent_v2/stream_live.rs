use super::stream::{Following, Frame};
use axum::response::sse::Event;
use layer2_core::sandbox::{AgentLiveDelivery, AgentLiveFragmentKind};
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

    pub(super) fn current_live_fragment(&self, fragment: &AgentLiveDelivery) -> bool {
        fragment.agent_id == self.header.id
            && Some(fragment.runtime_generation) == self.header.runtime_generation
            && Some(fragment.turn_sequence) == self.header.active_turn
            && (fragment.kind == AgentLiveFragmentKind::Thought
                || fragment.after_seq >= self.committed_message)
    }

    pub(super) fn queue_live(&mut self) {
        let mut fragments: Vec<AgentLiveDelivery> = Vec::new();
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
                        if previous.coalesce_legacy(&fragment, 16 * 1024) {
                            continue;
                        }
                    }
                    fragments.push(fragment);
                }
                Ok(fragment) => {
                    if self.state.event_wake
                        && fragment.agent_id == self.header.id
                        && Some(fragment.turn_sequence) == self.header.active_turn
                        && Some(fragment.runtime_generation) != self.header.runtime_generation
                    {
                        self.queue_fragments(&mut fragments);
                        self.queue_gap(Some(1), "lagged");
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(frames)) => {
                    self.queue_fragments(&mut fragments);
                    self.queue_gap(Some(frames), "lagged");
                }
                Err(_) => break,
            }
        }
        self.queue_fragments(&mut fragments);
    }

    fn queue_fragments(&mut self, fragments: &mut Vec<AgentLiveDelivery>) {
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
