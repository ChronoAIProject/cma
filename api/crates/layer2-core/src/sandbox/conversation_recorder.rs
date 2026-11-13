use std::sync::Arc;

use layer3_adapter::ConversationUpdate;

use crate::ids::AgentId;

use super::store::{AgentConversationUpdateStore, AgentStore, SandboxStoreError};
use super::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome, AgentUpdateRecorder,
};

const MAX_CONVERSATION_COMMIT_RETRIES: usize = 8;

#[path = "conversation_checkpoint.rs"]
mod checkpoint;

#[path = "conversation_budget.rs"]
mod budget;

#[path = "conversation_output.rs"]
mod output;
use output::store_error;

#[path = "conversation_record.rs"]
mod record;

#[path = "conversation_live.rs"]
pub(super) mod live;

#[path = "conversation_live_budget.rs"]
mod live_budget;
pub use live::{AgentLiveFragment, AgentLiveFragmentKind, AgentLiveSubscription};
pub use live_budget::AgentLiveDelivery;

pub struct DurableAgentUpdateRecorder {
    store: Arc<dyn AgentConversationUpdateStore>,
    live: live::AgentLiveHub,
    writer_policy: Arc<super::task_experience::TaskExperiencePolicy>,
    spend_source: Option<Arc<dyn crate::budget::SpendSource>>,
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for DurableAgentUpdateRecorder {
    async fn budget_stop(
        &self,
        id: &AgentId,
        runtime_generation: u64,
    ) -> Result<Option<crate::budget::BudgetStop>, AgentConversationError> {
        budget::stop(self.spend_source.as_deref(), id, runtime_generation).await
    }
    fn subscribe_live(&self, id: &AgentId) -> Option<AgentLiveSubscription> {
        Some(self.live.subscribe(id))
    }

    async fn restore_assembly(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
    ) -> Result<(), AgentConversationError> {
        if assembly.pending_update.is_some() {
            return Err(AgentConversationError::InvalidState(
                "an uncertain update must be retried before restoring its assembly",
            ));
        }
        for _ in 0..MAX_CONVERSATION_COMMIT_RETRIES {
            let snapshot = self
                .store
                .agent_message_snapshot(id)
                .await
                .map_err(store_error)?
                .ok_or(AgentConversationError::NotFound)?;
            let current = snapshot.agent;
            let previous = snapshot.checkpoint;
            if runtime_generation == 0
                || current.conversation.runtime_generation != Some(runtime_generation)
            {
                return Err(AgentConversationError::StaleGeneration);
            }
            if !current.status.is_live() {
                return Err(AgentConversationError::InvalidState(
                    "assembly restoration requires a live agent",
                ));
            }
            let recovered = match checkpoint::recover_assembly(
                self.store.as_ref(),
                &current,
                previous.as_ref(),
            )
            .await
            {
                Ok(recovered) => recovered,
                Err(error) => {
                    if AgentStore::get_agent(self.store.as_ref(), id)
                        .await
                        .map_err(store_error)?
                        .as_ref()
                        != Some(&current)
                    {
                        continue;
                    }
                    return Err(error);
                }
            };
            if let Some(next) = previous
                .as_ref()
                .and_then(|value| value.reanchored(&current))
            {
                match self
                    .store
                    .commit_agent_message_update(current, previous, next, None)
                    .await
                {
                    Ok(_) => {}
                    Err(SandboxStoreError::Conflict) => continue,
                    Err(error) => return Err(store_error(error)),
                }
            }
            *assembly = recovered;
            return Ok(());
        }
        Err(AgentConversationError::Unavailable(
            "conversation changed during every restoration attempt".into(),
        ))
    }

    async fn record_reopen_update(
        &self,
        id: &AgentId,
        guard: &super::store::AgentReopenGuard,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        self.record_with_reopen(
            id,
            guard.runtime_generation,
            assembly,
            update,
            appended_at,
            Some(guard),
        )
        .await
    }

    async fn record_update(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        self.record_with_reopen(id, runtime_generation, assembly, update, appended_at, None)
            .await
    }
}
