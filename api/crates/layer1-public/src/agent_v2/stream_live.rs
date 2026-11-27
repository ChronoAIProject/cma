use super::{
    cursor,
    error::Error,
    stream::{Following, Frame},
};
use axum::response::sse::Event;
use layer2_core::sandbox::{
    AgentLiveDelivery, AgentLiveEvent, AgentLiveFragmentKind, AgentLiveInterruption,
};
use serde_json::json;

impl Following {
    pub(super) fn queue_gap(&mut self, frames: Option<u64>, reason: &str) {
        self.pending.push_back(self.gap_frame(frames, reason));
    }

    pub(super) fn gap_frame(&self, frames: Option<u64>, reason: &str) -> Frame {
        let body = json!({"agent_id": self.header.id, "incarnation": self.identity,
            "turn_sequence": self.header.active_turn, "frames": frames, "reason": reason});
        Frame {
            event: Event::default()
                .event("cma.agent.live_gap")
                .data(body.to_string()),
            live: true,
            fragment: None,
        }
    }

    pub(super) fn current_live_fragment(&self, fragment: &AgentLiveDelivery) -> bool {
        fragment.agent_id == self.header.id
            && Some(fragment.runtime_generation) == self.header.runtime_generation
            && Some(fragment.turn_sequence) == self.header.active_turn
            && (fragment.kind == AgentLiveFragmentKind::Thought
                || fragment.after_seq >= self.committed_message)
    }

    pub(super) fn queue_live(&mut self, mut had_live_input: bool) {
        use tokio::sync::broadcast::error::TryRecvError;

        let owner = self
            .watching
            .as_ref()
            .and_then(|lane| lane.attached_owner());
        let mut fragments: Vec<AgentLiveDelivery> = Vec::new();
        for _ in 0..128 {
            let received = match self.held.take() {
                Some(fragment) => Ok(AgentLiveEvent::Fragment(fragment)),
                None => match &mut self.watching {
                    Some(watching) => watching.try_recv_event(),
                    None => break,
                },
            };
            match received {
                Ok(AgentLiveEvent::Fragment(fragment)) if self.current_live_fragment(&fragment) => {
                    had_live_input = true;
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
                Ok(AgentLiveEvent::Fragment(fragment)) => {
                    had_live_input = true;
                    if self.state.event_wake
                        && fragment.agent_id == self.header.id
                        && Some(fragment.turn_sequence) == self.header.active_turn
                        && Some(fragment.runtime_generation) != self.header.runtime_generation
                    {
                        self.queue_fragments(&mut fragments);
                        self.queue_gap(Some(1), "lagged");
                    }
                }
                Ok(AgentLiveEvent::Interrupted(interruption)) => {
                    self.queue_fragments(&mut fragments);
                    if had_live_input {
                        self.queue_gap(Some(1), "lagged");
                    } else {
                        self.interruption = Some(interruption);
                    }
                    break;
                }
                Err(TryRecvError::Lagged(frames)) => {
                    had_live_input = true;
                    self.queue_fragments(&mut fragments);
                    self.queue_gap(Some(frames), "lagged");
                }
                Err(TryRecvError::Empty) => {
                    if let Some(owner) = owner {
                        // Keep the first drained baseline for this owner. A
                        // peer retirement can still be in flight while durable
                        // catch-up reads a later, already idle head.
                        let bound = self
                            .watching
                            .as_ref()
                            .and_then(|lane| lane.attached_owner());
                        let new_owner = self
                            .drained_attachment
                            .as_ref()
                            .is_none_or(|(previous, _)| *previous != owner);
                        if new_owner && bound.as_ref() == Some(&owner) {
                            self.drained_attachment = Some((owner, self.header.clone()));
                        }
                    }
                    break;
                }
                Err(TryRecvError::Closed) => break,
            }
        }
        self.queue_fragments(&mut fragments);
    }

    pub(super) async fn reattach(
        &mut self,
        interruption: AgentLiveInterruption,
    ) -> Result<(), Error> {
        if tokio::time::Instant::now() < interruption.deadline
            && self.after == self.header.head
            && self.header.runtime_generation.is_some()
            && self.header.active_turn.is_none()
            && self
                .drained_attachment
                .as_ref()
                .is_some_and(|(owner, header)| {
                    *owner == interruption.owner && *header == self.header
                })
            && self.held.is_none()
            && self.deferred_live.is_empty()
            && self.discarded_fragments == 0
        {
            let baseline = &self.header;
            let proof = async {
                let Some(lane) = &mut self.watching else {
                    return Ok(None);
                };
                let Some(successor) = lane.successor(&interruption).await else {
                    return Ok(None);
                };
                // The successor is already subscribed before the authorized read.
                let current = self
                    .state
                    .reads
                    .event_stream_header(&self.authorization)
                    .await?;
                if cursor::incarnation(&current) != self.identity {
                    return Err(Error::Incarnation);
                }
                if current == *baseline && successor.confirm().await && lane.is_empty() {
                    Ok(Some(successor.owner().clone()))
                } else {
                    Ok(None)
                }
            };
            match tokio::time::timeout_at(interruption.deadline, proof).await {
                Ok(Ok(Some(owner))) if tokio::time::Instant::now() < interruption.deadline => {
                    self.drained_attachment = Some((owner, self.header.clone()));
                    return Ok(());
                }
                Ok(Err(Error::Incarnation | Error::Missing)) => {
                    self.ended = true;
                    return Ok(());
                }
                _ => (),
            }
        }

        // Keep the cursor and any queued durable prefix. The normal catch-up
        // and release checks run before this gap can reach the caller.
        self.discarded_fragments += 1;
        self.needs_catchup = true;
        Ok(())
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
