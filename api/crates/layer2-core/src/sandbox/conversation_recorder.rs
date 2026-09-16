use std::sync::Arc;

use layer3_adapter::ConversationUpdate;
use sha2::{Digest, Sha256};

use crate::ids::AgentId;
use crate::seconds_from_rfc3339;

use super::conversation::PendingAgentUpdate;
use super::conversation_fold::{fold_update, FoldedConversationUpdate};
use super::store::{
    AgentConversationMutation, AgentConversationUpdateStore, AgentMessageCheckpoint, AgentStore,
    SandboxStoreError,
};
use super::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome, AgentUpdateRecorder,
};

const MAX_CONVERSATION_COMMIT_RETRIES: usize = 8;

#[path = "conversation_live.rs"]
pub(super) mod live;

pub use live::{AgentLiveFragment, AgentLiveFragmentKind, AgentLiveSubscription};

pub struct DurableAgentUpdateRecorder {
    store: Arc<dyn AgentConversationUpdateStore>,
    live: live::AgentLiveHub,
}

impl DurableAgentUpdateRecorder {
    pub fn new<S>(store: Arc<S>) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Arc::new(Self {
            store,
            live: live::AgentLiveHub::default(),
        })
    }
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for DurableAgentUpdateRecorder {
    fn subscribe_live(&self, id: &AgentId) -> Option<AgentLiveSubscription> {
        Some(self.live.subscribe(id))
    }

    async fn record_update(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        if runtime_generation == 0 {
            return Err(AgentConversationError::StaleGeneration);
        }
        if seconds_from_rfc3339(&appended_at).is_none() {
            return Err(AgentConversationError::InvalidState(
                "conversation update timestamp is invalid",
            ));
        }
        let fingerprint = format!("{:x}", Sha256::digest(format!("{update:?}").as_bytes()));
        let update_id = if let Some(pending) = &assembly.pending_update {
            if pending.fingerprint != fingerprint
                || pending.runtime_generation != runtime_generation
            {
                return Err(AgentConversationError::InvalidState(
                    "an uncertain update must be retried exactly",
                ));
            }
            pending.id.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        for _ in 0..MAX_CONVERSATION_COMMIT_RETRIES {
            let current = AgentStore::get_agent(self.store.as_ref(), id)
                .await
                .map_err(store_error)?
                .ok_or(AgentConversationError::NotFound)?;
            if current.conversation.runtime_generation != Some(runtime_generation) {
                return Err(AgentConversationError::StaleGeneration);
            }
            let previous = self
                .store
                .agent_message_checkpoint(id)
                .await
                .map_err(store_error)?;
            if let Some(recorded) = previous
                .as_ref()
                .filter(|checkpoint| checkpoint.update_id == update_id)
            {
                if recorded.update_fingerprint != fingerprint {
                    return Err(AgentConversationError::Corrupt(
                        "checkpoint update identity conflict".into(),
                    ));
                }
                *assembly = if recorded.matches(&current) {
                    recorded.assembly.clone()
                } else {
                    AgentMessageAssembly::default()
                };
                return Ok(recorded.outcome.clone());
            }
            if let Some(pending) = &assembly.pending_update {
                if pending.active_turn != current.conversation.active_turn
                    || pending.after_seq != current.conversation.last_seq
                {
                    return Err(AgentConversationError::Unavailable(
                        "uncertain update receipt is no longer the current checkpoint".into(),
                    ));
                }
            }
            let recovered = previous
                .as_ref()
                .filter(|checkpoint| checkpoint.matches(&current))
                .map(|checkpoint| checkpoint.assembly.clone())
                .unwrap_or_default();
            if previous.as_ref().is_some_and(|checkpoint| {
                checkpoint.native_conversation_id
                    == current
                        .conversation
                        .conversation_id
                        .as_deref()
                        .unwrap_or_default()
                    && checkpoint.runtime_generation == runtime_generation
                    && checkpoint.active_turn == current.conversation.active_turn
                    && checkpoint.after_seq != current.conversation.last_seq
                    && !checkpoint.assembly.is_empty()
            }) {
                // A generic semantic write advances the private anchor atomically, but
                // these two reads are separate. Retry a changed aggregate before treating
                // a stable, inconsistent checkpoint as corrupt.
                let refreshed = AgentStore::get_agent(self.store.as_ref(), id)
                    .await
                    .map_err(store_error)?;
                if refreshed.as_ref() != Some(&current) {
                    continue;
                }
                return Err(AgentConversationError::Corrupt(
                    "active message checkpoint has a stale log anchor".into(),
                ));
            }
            let folded = fold_update(&current, &recovered, update.clone(), &appended_at)?;
            let (next_agent, mut next_assembly, logs, turn_closed) = match folded {
                FoldedConversationUpdate::Quiet(next) => (current.clone(), next, Vec::new(), false),
                FoldedConversationUpdate::Changed(change) => (
                    change.agent,
                    change.assembly,
                    change.logs,
                    change.turn_closed,
                ),
            };
            let fragment = live::accepted_fragment(
                &current,
                &recovered,
                &mut next_assembly,
                &update,
                next_agent.conversation.last_seq,
            );
            // Thoughts remain ephemeral, and an unchanged bounded message needs no write.
            // Accepted assistant text is checkpointed before it can be published live.
            let durable_assembly_changed = next_assembly.message_id != recovered.message_id
                || next_assembly.content != recovered.content
                || next_assembly.prompt_evidence != recovered.prompt_evidence;
            if logs.is_empty() && !durable_assembly_changed {
                *assembly = next_assembly;
                self.live.publish(fragment);
                return Ok(AgentUpdateOutcome::quiet());
            }
            let outcome = AgentUpdateOutcome {
                appended: logs
                    .iter()
                    .enumerate()
                    .map(|(index, kind)| super::AgentLogEntry {
                        agent_id: id.clone(),
                        seq: current.conversation.last_seq + index as u64 + 1,
                        appended_at: appended_at.clone(),
                        kind: kind.clone(),
                    })
                    .collect(),
                turn_closed,
            };
            let checkpoint = AgentMessageCheckpoint {
                version: 1,
                update_id: update_id.clone(),
                update_fingerprint: fingerprint.clone(),
                native_conversation_id: current.conversation.conversation_id.clone().ok_or(
                    AgentConversationError::InvalidState("conversation identity is missing"),
                )?,
                runtime_generation,
                active_turn: next_agent.conversation.active_turn,
                after_seq: next_agent.conversation.last_seq,
                assembly: next_assembly.clone(),
                outcome,
            };
            checkpoint.validate().map_err(store_error)?;
            let mutation = (!logs.is_empty()).then(|| AgentConversationMutation {
                command_claim: None,
                execution_fence: None,
                channel_control_execution_fence: None,
                channel_execution_fence: None,
                trigger_execution_fence: None,
                admission_guard: None,
                expected_agent_revision: current.revision,
                expected_last_seq: current.conversation.last_seq,
                runtime_generation,
                agent: next_agent,
                appended_at: appended_at.clone(),
                agent_logs: logs,
            });
            match self
                .store
                .commit_agent_message_update(current.clone(), previous, checkpoint, mutation)
                .await
            {
                Ok(outcome) => {
                    *assembly = next_assembly;
                    self.live.publish(fragment);
                    return Ok(outcome);
                }
                Err(SandboxStoreError::Conflict) => {}
                Err(SandboxStoreError::SequenceConflict) => {
                    return Err(AgentConversationError::Corrupt(
                        "agent log sequence is inconsistent".into(),
                    ));
                }
                Err(error @ SandboxStoreError::Unavailable(_)) => {
                    assembly.pending_update = Some(Box::new(PendingAgentUpdate {
                        id: update_id.clone(),
                        fingerprint: fingerprint.clone(),
                        runtime_generation,
                        active_turn: current.conversation.active_turn,
                        after_seq: current.conversation.last_seq,
                    }));
                    return Err(store_error(error));
                }
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(AgentConversationError::Unavailable(
            "conversation changed during every commit attempt".into(),
        ))
    }
}

fn store_error(error: SandboxStoreError) -> AgentConversationError {
    match error {
        SandboxStoreError::NotFound => AgentConversationError::NotFound,
        SandboxStoreError::AgentLogCapacityExceeded => AgentConversationError::CapacityExceeded,
        SandboxStoreError::Corrupt(detail) => AgentConversationError::Corrupt(detail),
        other => AgentConversationError::Unavailable(other.to_string()),
    }
}
