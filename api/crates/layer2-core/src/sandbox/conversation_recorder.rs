use std::sync::Arc;

use layer3_adapter::ConversationUpdate;

use crate::ids::AgentId;

use super::store::AgentConversationUpdateStore;
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
    AgentLiveError, AgentLiveFragment, AgentLiveFragmentKind, AgentLivePublisher, AgentLiveSource,
    AgentLiveSubscription,
};
pub use live_budget::AgentLiveDelivery;

pub struct DurableAgentUpdateRecorder {
    store: Arc<dyn AgentConversationUpdateStore>,
    live: live::AgentLiveHub,
    live_source: std::sync::OnceLock<Arc<dyn AgentLiveSource>>,
    writer_policy: Arc<super::task_experience::TaskExperiencePolicy>,
    spend_source: Option<Arc<dyn crate::budget::SpendSource>>,
    coalesce_policy: Option<AgentOutputCoalescePolicy>,
}

mod configuration;
mod ingress;

#[cfg(test)]
mod ingress_cancellation_tests;
#[cfg(test)]
mod ingress_compat_tests;
#[cfg(test)]
mod ingress_session_tests;

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
        checkpoint::restore(self, id, runtime_generation, assembly).await
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
        if update.delivery().receipt.is_none() && assembly.buffered_output().is_some() {
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
