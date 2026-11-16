//! Shared observation projects one source event at a time. Text retains its
//! separately bounded original payload while emitting only one chunk per poll.
use super::*;

#[derive(Default)]
pub(super) struct ReplayQueue {
    pub sources: VecDeque<layer2_core::sandbox::AgentVisibleLogEntry>,
    text: Option<Text>,
}
struct Text {
    content: String,
    offset: usize,
    stage: u8,
    event: AguiEvent,
    role: &'static str,
    sequence: u64,
}
impl ReplayQueue {
    pub fn empty(&self) -> bool {
        self.sources.is_empty() && self.text.is_none()
    }
    pub fn begin_text(&mut self, entry: &mut AgentLogEntry, turn: u64) -> bool {
        let (content, role) = match &mut entry.kind {
            AgentLogKind::OwnerMessage { content } => (std::mem::take(content), "user"),
            AgentLogKind::AgentMessage { content } => (std::mem::take(content), "assistant"),
            _ => return false,
        };
        self.text = Some(Text {
            content,
            offset: 0,
            stage: 0,
            role,
            sequence: entry.seq,
            event: AguiEvent {
                kind: "TEXT_MESSAGE_START",
                thread_id: entry.agent_id.to_string(),
                run_id: turn.to_string(),
                body: serde_json::Value::Null,
                timestamp: Some(entry.appended_at.clone()),
                seq: None,
                live: false,
            },
        });
        true
    }
    pub fn next_text(&mut self) -> Option<AguiEvent> {
        let text = self.text.as_mut()?;
        let mut event = text.event.clone();
        let id = format!("msg_{}", text.sequence);
        match text.stage {
            0 => {
                event.body = serde_json::json!({"messageId": id, "role": text.role});
                text.stage = 1;
            }
            1 => {
                let mut end = (text.offset + 8192).min(text.content.len());
                while !text.content.is_char_boundary(end) {
                    end -= 1;
                }
                event.kind = "TEXT_MESSAGE_CONTENT";
                event.body =
                    serde_json::json!({"messageId": id, "delta": &text.content[text.offset..end]});
                text.offset = end;
                if end == text.content.len() {
                    text.stage = 2;
                }
            }
            _ => {
                event.kind = "TEXT_MESSAGE_END";
                event.seq = Some(text.sequence);
                event.body = serde_json::json!({"messageId": id});
                self.text = None;
            }
        }
        Some(event)
    }
}

impl Following {
    pub(super) fn prepare_shared_frame(
        &mut self,
    ) -> Result<(), layer2_core::sandbox::AgentReadError> {
        if self.observation.is_none() || !self.pending.is_empty() {
            return Ok(());
        }
        if let Some(frame) = self.replay_queue.next_text() {
            self.pending.push_back(frame);
            return Ok(());
        }
        if let Some(entry) = self.replay_queue.sources.pop_front() {
            if let Some(kind) = entry.kind {
                let mut entry = AgentLogEntry {
                    agent_id: self.id.clone(),
                    seq: entry.seq,
                    appended_at: entry.appended_at,
                    kind,
                };
                if let AgentLogKind::TurnStarted { turn_sequence, .. } = entry.kind {
                    if self.run.is_some_and(|run| run != turn_sequence) {
                        self.ended = true;
                        self.replay_queue.sources.clear();
                        return Ok(());
                    }
                    self.turn = turn_sequence;
                }
                self.after = entry.seq;
                self.approvals.observe(&entry);
                if matches!(entry.kind, AgentLogKind::AgentMessage { .. }) {
                    self.committed_message_seq = entry.seq;
                }
                if self.replay_queue.begin_text(&mut entry, self.turn) {
                    self.pending
                        .push_back(self.replay_queue.next_text().unwrap());
                } else {
                    let events = project(&entry, self.turn);
                    let bytes = events
                        .iter()
                        .try_fold(0_usize, |total, event| {
                            serde_json::to_vec(event).map(|value| total + value.len())
                        })
                        .map_err(|error| {
                            layer2_core::sandbox::AgentReadError::Corrupt(error.to_string())
                        })?;
                    if events.len() > 200 || bytes > 4 * 1024 * 1024 {
                        return Err(layer2_core::sandbox::AgentReadError::Unavailable(
                            "replay payload exceeds the frame budget".into(),
                        ));
                    }
                    self.pending.extend(events);
                }
                if self.run.is_some() && ending(&entry.kind) {
                    self.ended = true;
                    self.replay_queue.sources.clear();
                }
            } else {
                self.after = entry.seq;
                self.pending.push_back(AguiEvent {
                    kind: "STATE_DELTA",
                    thread_id: self.id.to_string(),
                    run_id: self.turn.to_string(),
                    body: serde_json::json!({"delta": []}),
                    timestamp: Some(entry.appended_at),
                    seq: Some(entry.seq),
                    live: false,
                });
            }
        } else if self
            .header
            .as_ref()
            .is_some_and(|header| self.after == header.head)
        {
            let pending = self.approvals.clone().into_entries();
            self.interrupt_current(
                self.header.as_ref().and_then(|header| header.active_turn),
                &pending,
            );
        }
        if self.replay_queue.empty() && self.through.is_some_and(|through| self.after >= through) {
            self.ended = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lazy_text_is_identical_to_bounded_projection_including_empty_unicode_and_reconnect_cursor() {
        for content in [
            String::new(),
            "😀\n".repeat(20000),
            "large".repeat(1024 * 1024),
        ] {
            let mut entry = AgentLogEntry {
                agent_id: AgentId("agent".into()),
                seq: 17,
                appended_at: "2026-11-16T00:00:00Z".into(),
                kind: AgentLogKind::AgentMessage { content },
            };
            let expected = project(&entry, 3);
            let mut queue = ReplayQueue::default();
            assert!(queue.begin_text(&mut entry, 3));
            for (index, expected_frame) in expected.iter().enumerate() {
                let frame = queue.next_text().unwrap();
                assert_eq!(&frame, expected_frame);
                assert!(serde_json::to_vec(&frame).unwrap().len() < 64 * 1024);
                assert_eq!(frame.seq, (index + 1 == expected.len()).then_some(17));
            }
            assert!(queue.empty());
        }
    }
}
