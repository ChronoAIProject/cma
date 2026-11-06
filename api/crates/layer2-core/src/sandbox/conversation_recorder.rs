use std::sync::Arc;

use layer3_adapter::ConversationUpdate;

use crate::ids::AgentId;

use super::store::{
    AgentConversationUpdateStore, AgentMessageCheckpoint, AgentStore, SandboxStoreError,
};
use super::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome, AgentUpdateRecorder,
};

const MAX_CONVERSATION_COMMIT_RETRIES: usize = 8;

#[path = "conversation_checkpoint.rs"]
mod checkpoint;

#[path = "conversation_output.rs"]
mod output;
use output::output_replayed;

#[path = "conversation_live.rs"]
pub(super) mod live;

#[path = "conversation_reconcile.rs"]
mod reconcile;

#[path = "conversation_record.rs"]
mod record;

pub use live::{AgentLiveFragment, AgentLiveFragmentKind, AgentLiveSubscription};

pub struct DurableAgentUpdateRecorder {
    store: Arc<dyn AgentConversationUpdateStore>,
    live: live::AgentLiveHub,
    writer_policy: Arc<super::task_experience::TaskExperiencePolicy>,
}

impl DurableAgentUpdateRecorder {
    pub fn new<S>(store: Arc<S>) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Self::with_writer_policy(store, Arc::default())
    }

    pub fn with_writer_policy<S>(
        store: Arc<S>,
        writer_policy: Arc<super::task_experience::TaskExperiencePolicy>,
    ) -> Arc<Self>
    where
        S: AgentConversationUpdateStore + 'static,
    {
        Arc::new(Self {
            store,
            writer_policy,
            live: live::AgentLiveHub::default(),
        })
    }
}

#[async_trait::async_trait]
impl AgentUpdateRecorder for DurableAgentUpdateRecorder {
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
            let current = AgentStore::get_agent(self.store.as_ref(), id)
                .await
                .map_err(store_error)?
                .ok_or(AgentConversationError::NotFound)?;
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
            let previous = self
                .store
                .agent_message_checkpoint(id)
                .await
                .map_err(store_error)?;
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

    async fn reconcile_completion(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        evidence: super::NativeCompletionEvidence,
        response: layer3_adapter::PromptResponse,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        DurableAgentUpdateRecorder::reconcile_completion(
            self,
            id,
            runtime_generation,
            assembly,
            evidence,
            response,
            appended_at,
        )
        .await
    }
}

pub(super) fn store_error(error: SandboxStoreError) -> AgentConversationError {
    match error {
        SandboxStoreError::NotFound => AgentConversationError::NotFound,
        SandboxStoreError::InvalidTransaction(detail) => {
            AgentConversationError::InvalidState(detail)
        }
        SandboxStoreError::AgentLogCapacityExceeded => AgentConversationError::CapacityExceeded,
        SandboxStoreError::Corrupt(detail) => AgentConversationError::Corrupt(detail),
        other => AgentConversationError::Unavailable(other.to_string()),
    }
}
