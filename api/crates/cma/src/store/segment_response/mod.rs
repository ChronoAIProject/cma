//! Read-only legacy response window using the shared prepared integrity proof.
use std::sync::Arc;

use layer2_core::sandbox::{
    store::{AgentResponseLogStore, AgentResponseLogWindow, SandboxStoreError},
    Agent, AgentLogKind,
};

use super::{
    agent_log_reads::{
        integrity::{LogReadIntegrity, PreparedIntegrity},
        read_bounded,
    },
    sandbox_mongo::MongoSandboxStore,
};

const MAX_ENTRIES: u64 = 200;
const MAX_BYTES: usize = 4 * 1024 * 1024;

impl MongoSandboxStore {
    pub(crate) fn with_bounded_response_reads(mut self, enabled: bool) -> Self {
        self.bounded_response_reads = enabled;
        self
    }

    pub(crate) fn response_log_reader(self: &Arc<Self>) -> Option<Arc<dyn AgentResponseLogStore>> {
        self.bounded_response_reads
            .then(|| self.clone() as Arc<dyn AgentResponseLogStore>)
    }
}

#[async_trait::async_trait]
impl AgentResponseLogStore for MongoSandboxStore {
    async fn agent_response_log_window(
        &self,
        agent: &Agent,
        turn_sequence: Option<u64>,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
        if !self.bounded_response_reads {
            return Ok(None);
        }
        let mut session = self
            .client
            .start_session()
            .snapshot(true)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let head = PreparedIntegrity
            .read_head(self, &agent.id, &mut session)
            .await?
            .ok_or(SandboxStoreError::NotFound)?;
        if head.last_seq != agent.conversation.last_seq {
            return Err(SandboxStoreError::SequenceConflict);
        }
        if !head.trusted {
            return Ok(None);
        }
        let mut entries = Vec::new();
        if let Some(turn) = turn_sequence {
            if turn == 0 || turn > head.last_seq {
                return Err(SandboxStoreError::SequenceConflict);
            }
            let through = head.last_seq.min(turn.saturating_add(MAX_ENTRIES - 1));
            let Some(window) = read_bounded(
                self,
                &agent.id,
                turn - 1,
                through,
                &head,
                &mut session,
                MAX_BYTES,
            )
            .await?
            else {
                return Ok(None);
            };
            if !window.first().is_some_and(|entry| {
                matches!(&entry.kind,
                AgentLogKind::TurnStarted { turn_sequence, model: Some(_), .. }
                    if *turn_sequence == turn)
            }) {
                // A historical TurnStarted without a retained model needs its
                // preceding settings history. Today's model is not evidence.
                return Ok(None);
            }
            let next_turn = window
                .iter()
                .skip(1)
                .position(|entry| matches!(entry.kind, AgentLogKind::TurnStarted { .. }));
            if next_turn.is_none() && through != head.last_seq {
                return Ok(None);
            }
            entries = window;
            if let Some(index) = next_turn {
                entries.truncate(index + 2);
            }
        }
        tracing::debug!(
            source_rows = entries.len(),
            source_head = head.last_seq,
            "bounded response source avoided a complete prefix"
        );
        Ok(Some(AgentResponseLogWindow {
            through: head.last_seq,
            turn_sequence,
            entries,
        }))
    }
}
