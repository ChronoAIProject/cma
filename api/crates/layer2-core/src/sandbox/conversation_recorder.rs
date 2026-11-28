use std::sync::Arc;

use layer3_adapter::ConversationUpdate;

use crate::ids::AgentId;

use super::store::{AgentConversationUpdateStore, AgentStore, SandboxStoreError};
use super::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome, AgentUpdateRecorder,
};

#[path = "conversation_coalesce.rs"]
pub(crate) mod coalesce;
use super::AgentOutputCommit;
pub use coalesce::AgentOutputCoalescePolicy;
use coalesce::RecordInput;

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
pub use live::AgentHeadSubscription;
pub use live::{
    AgentLiveError, AgentLiveEvent, AgentLiveFragment, AgentLiveFragmentKind,
    AgentLiveInterruption, AgentLivePublisher, AgentLiveSource, AgentLiveSubscription,
};
pub use live_budget::AgentLiveDelivery;

pub struct DurableAgentUpdateRecorder {
    store: Arc<dyn AgentConversationUpdateStore>,
    live: live::AgentLiveHub,
    live_source: std::sync::OnceLock<Arc<dyn AgentLiveSource>>,
    writer_policy: Arc<super::task_experience::TaskExperiencePolicy>,
    spend_source: Option<Arc<dyn crate::budget::SpendSource>>,
    coalesce_policy: Option<AgentOutputCoalescePolicy>,
    incremental_text: bool,
}

mod configuration;

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
        match self.live_source.get() {
            Some(source) => source.subscribe(id),
            None => Some(self.subscribe_local(id)),
        }
    }
    async fn admit_live(
        &self,
        id: &AgentId,
    ) -> Result<Option<AgentLiveSubscription>, AgentLiveError> {
        let mut subscription = match self.live_source.get() {
            Some(source) => source.try_subscribe(id)?,
            None => Some(self.subscribe_local(id)),
        };
        if let Some(subscription) = subscription.as_mut() {
            // Public stream and command admission call this before accepting
            // headers; local subscriptions have no admission receiver and
            // therefore return immediately.
            subscription.admit().await?;
        }
        Ok(subscription)
    }
    fn subscribe_head(&self, id: &AgentId) -> Option<AgentHeadSubscription> {
        Some(self.live.subscribe_head(id))
    }

    async fn restore_assembly(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
    ) -> Result<(), AgentConversationError> {
        if assembly.pending_update.is_some() || assembly.buffered_output().is_some() {
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
            RecordInput::Update(&update, false),
            appended_at,
            Some(guard),
        )
        .await
        .map(|result| result.outcome)
    }

    async fn record_update(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        self.record_with_reopen(
            id,
            runtime_generation,
            assembly,
            RecordInput::Update(&update, false),
            appended_at,
            None,
        )
        .await
        .map(|result| result.outcome)
    }
    async fn record_output(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentOutputCommit, AgentConversationError> {
        if !matches!(update, ConversationUpdate::RuntimeOutput { .. })
            && assembly.buffered_output().is_some()
        {
            return Err(AgentConversationError::InvalidState(
                "flush quiet receipts before a legacy update",
            ));
        }
        self.record_with_reopen(
            id,
            runtime_generation,
            assembly,
            RecordInput::Update(&update, true),
            appended_at,
            None,
        )
        .await
    }

    async fn flush_output(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        appended_at: String,
    ) -> Result<Option<layer3_adapter::RuntimeOutputSource>, AgentConversationError> {
        let Some(source) = assembly.buffered_output().cloned() else {
            return Ok(None);
        };
        self.record_with_reopen(
            id,
            runtime_generation,
            assembly,
            RecordInput::Flush(&source),
            appended_at,
            None,
        )
        .await
        .map(|result| result.committed)
    }
}
