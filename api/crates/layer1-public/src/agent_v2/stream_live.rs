use super::stream::{Following, Frame};
use axum::response::sse::Event;
use layer2_core::sandbox::AgentLiveFragmentKind;
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
            fragment: false,
        });
    }

    pub(super) fn queue_live(&mut self) {
        for _ in 0..32 {
            let received = match self.held.take() {
                Some(fragment) => Ok(fragment),
                None => match &mut self.watching {
                    Some(watching) => watching.try_recv(),
                    None => break,
                },
            };
            match received {
                Ok(fragment)
                    if fragment.agent_id == self.header.id
                        && Some(fragment.runtime_generation) == self.header.runtime_generation
                        && Some(fragment.turn_sequence) == self.header.active_turn
                        && (fragment.kind == AgentLiveFragmentKind::Thought
                            || fragment.after_seq >= self.committed_message) =>
                {
                    if fragment.after_seq > self.header.head {
                        self.held = Some(fragment);
                        break;
                    }
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
                        fragment: true,
                    });
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(frames)) => {
                    self.queue_gap(Some(frames), "lagged")
                }
                Err(_) => break,
            }
        }
    }
}
