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
                let FoldedConversationUpdate::Quiet(mut next_assembly) = folded else {
                    unreachable!()
                };
                let fragment = live::accepted_fragment(
                    &current,
                    assembly,
                    &mut next_assembly,
                    &update,
                    current.conversation.last_seq,
                );
                *assembly = next_assembly;
                self.live.publish(fragment);
                return Ok(AgentUpdateOutcome::quiet());
            };
            let mut change = *change;
            let fragment = live::accepted_fragment(
                &current,
                assembly,
                &mut change.assembly,
                &update,
                change.agent.conversation.last_seq,
            );
            let mutation = AgentConversationMutation {
                admission_guard: None,
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
                    self.live.publish(fragment);
                    return Ok(AgentUpdateOutcome {
                        appended,
                        turn_closed: change.turn_closed,
                    });
                }
                Err(SandboxStoreError::Conflict) => {}
                Err(SandboxStoreError::SequenceConflict) => {
                    return Err(AgentConversationError::Corrupt(
                        "agent log sequence is inconsistent".into(),
                    ));
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
