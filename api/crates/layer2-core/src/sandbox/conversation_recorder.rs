use std::sync::Arc;

use layer3_adapter::ConversationUpdate;

use crate::ids::AgentId;
use crate::seconds_from_rfc3339;

use super::conversation_fold::{fold_update, FoldedConversationUpdate};
use super::store::{
    AgentConversationMutation, AgentConversationUpdateStore, AgentStore, SandboxStoreError,
};
use super::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome, AgentUpdateRecorder,
};

const MAX_CONVERSATION_COMMIT_RETRIES: usize = 8;

pub struct DurableAgentUpdateRecorder {
    store: Arc<dyn AgentConversationUpdateStore>,
}

impl DurableAgentUpdateRecorder {
    pub fn new<S>(store: Arc<S>) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Arc::new(Self { store })
    }
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for DurableAgentUpdateRecorder {
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
        for _ in 0..MAX_CONVERSATION_COMMIT_RETRIES {
            let current = AgentStore::get_agent(self.store.as_ref(), id)
                .await
                .map_err(store_error)?
                .ok_or(AgentConversationError::NotFound)?;
            if current.conversation.runtime_generation != Some(runtime_generation) {
                return Err(AgentConversationError::StaleGeneration);
            }
            let folded = fold_update(&current, assembly, update.clone(), &appended_at)?;
            let FoldedConversationUpdate::Changed(change) = folded else {
                let FoldedConversationUpdate::Quiet(next_assembly) = folded else {
                    unreachable!()
                };
                *assembly = next_assembly;
                return Ok(AgentUpdateOutcome::quiet());
            };
            let change = *change;
            let mutation = AgentConversationMutation {
                expected_agent_revision: current.revision,
                expected_last_seq: current.conversation.last_seq,
                runtime_generation,
                agent: change.agent,
                appended_at: appended_at.clone(),
                agent_logs: change.logs,
            };
            match self.store.commit_agent_conversation(mutation).await {
                Ok(appended) => {
                    *assembly = change.assembly;
                    return Ok(AgentUpdateOutcome {
                        appended,
                        turn_closed: change.turn_closed,
                    });
                }
                Err(SandboxStoreError::Conflict | SandboxStoreError::SequenceConflict) => {}
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
        SandboxStoreError::Corrupt(detail) => AgentConversationError::Corrupt(detail),
        other => AgentConversationError::Unavailable(other.to_string()),
    }
}
