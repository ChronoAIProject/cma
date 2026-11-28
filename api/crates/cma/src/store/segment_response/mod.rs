//! Read-only legacy response window using the shared prepared integrity proof.
use std::sync::Arc;

use layer2_core::sandbox::{
    store::{AgentResponseLogStore, AgentResponseLogWindow, SandboxStoreError},
    Agent, AgentLogKind,
};

use super::{
    agent_log_reads::{integrity::read_response_head, read_bounded},
    sandbox_mongo::MongoSandboxStore,
};

const MAX_ENTRIES: u64 = 200;
const MAX_BYTES: usize = 4 * 1024 * 1024;

mod advance;
mod boundary;
mod indexed;
mod suffix;
mod telemetry;
pub(crate) use telemetry::emit;
use telemetry::{Attempt, Outcome};

impl MongoSandboxStore {
    pub(crate) fn with_bounded_response_reads(mut self, enabled: bool) -> Self {
        self.bounded_response_reads = enabled;
        self
    }

    pub(crate) fn response_log_reader(self: &Arc<Self>) -> Option<Arc<dyn AgentResponseLogStore>> {
        // The optional optimization applies to legacy records. Adopted records
        // must keep their compatible reader when writer/admission flags close.
        Some(self.clone() as Arc<dyn AgentResponseLogStore>)
    }
}

#[async_trait::async_trait]
impl AgentResponseLogStore for MongoSandboxStore {
    async fn agent_output_available(&self, agent: &Agent) -> Result<bool, SandboxStoreError> {
        let row = self
            .agents
            .find_one(mongodb::bson::doc! {"_id":agent.id.as_str()})
            .await
            .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?
            .ok_or(SandboxStoreError::NotFound)?;
        Ok(super::agent_log_segments::read_format(&row)?.is_some())
    }
    async fn agent_output_reader(
        &self,
        agent: &Agent,
        turn: Option<u64>,
    ) -> Result<
        Option<Box<dyn layer2_core::sandbox::store::indexed::AgentOutputReader + '_>>,
        SandboxStoreError,
    > {
        indexed::open(self, agent, turn).await
    }

    async fn agent_response_log_suffix(
        &self,
        agent: &Agent,
        previous: &AgentResponseLogWindow,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
        suffix::read(self, agent, previous).await
    }

    async fn agent_response_log_window(
        &self,
        agent: &Agent,
        turn_sequence: Option<u64>,
    ) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
        if !self.bounded_response_reads {
            return Ok(None);
        }
        let mut attempt = Attempt::new();
        let mut session = self
            .client
            .start_session()
            .snapshot(true)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let (head, captured) = read_response_head(self, &agent.id, &mut session).await?;
        if captured.revision == agent.revision && captured != *agent {
            return Err(SandboxStoreError::SequenceConflict);
        }
        if head.last_seq != agent.conversation.last_seq {
            return Err(advance::legacy(
                self,
                agent,
                &captured,
                &head,
                turn_sequence,
                None,
                &mut session,
            )
            .await?);
        }
        if !head.trusted {
            attempt.finish(Outcome::Untrusted);
            return Ok(None);
        }
        let mut entries = Vec::new();
        if let Some(turn) = turn_sequence {
            if turn == 0 || turn > head.last_seq {
                return Err(SandboxStoreError::SequenceConflict);
            }
            let through = head.last_seq.min(turn.saturating_add(MAX_ENTRIES - 1));
            let next_turn =
                boundary::next_turn(self, &agent.id, turn, through, &mut session).await?;
            if next_turn.is_none() && through != head.last_seq {
                attempt.finish(Outcome::Budget);
                return Ok(None);
            }
            // The witness belongs to this response's proof; payloads after it
            // must not spend this response's budget or force lifetime fallback.
            let through = next_turn.unwrap_or(through);
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
                attempt.finish(Outcome::Budget);
                return Ok(None);
            };
            if !window.first().is_some_and(|entry| {
                matches!(&entry.kind,
                AgentLogKind::TurnStarted { turn_sequence, model: Some(_), .. }
                    if *turn_sequence == turn)
            }) {
                // A historical TurnStarted without a retained model needs its
                // preceding settings history. Today's model is not evidence.
                attempt.finish(Outcome::Model);
                return Ok(None);
            }
            if next_turn.is_some()
                && !window.last().is_some_and(|entry| {
                    matches!(entry.kind, AgentLogKind::TurnStarted { turn_sequence, .. }
                        if turn_sequence == through)
                })
            {
                return Err(SandboxStoreError::SequenceConflict);
            }
            entries = window;
        }
        attempt.finish(Outcome::Bounded);
        Ok(Some(AgentResponseLogWindow {
            through: head.last_seq,
            turn_sequence,
            entries,
        }))
    }
}
