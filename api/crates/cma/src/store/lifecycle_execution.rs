use layer2_core::ids::SandboxId;
use layer2_core::sandbox::store::{
    AgentConversationStore, AgentMessageSnapshot, Page, PageRequest, ReopenRefusalFence,
    SandboxCheckpointMutation, SandboxLifecycleExecutionStore, SandboxLifecycleMutation,
    SandboxStoreError, MAX_PAGE_SIZE,
};
use layer2_core::sandbox::Agent;
use mongodb::bson::doc;

use super::agents::read_agent;
use super::sandbox_mongo::MongoSandboxStore;
mod cleanup;

pub(super) enum LifecycleLoadError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

#[async_trait::async_trait]
impl SandboxLifecycleExecutionStore for MongoSandboxStore {
    async fn retired_runtime_cleanup_plan(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
    ) -> Result<layer2_core::sandbox::store::RetiredRuntimeCleanupPlan, SandboxStoreError> {
        cleanup::paths(self, sandbox).await
    }

    async fn retired_agent_runtime_paths(
        &self,
        sandbox: &layer2_core::sandbox::Sandbox,
    ) -> Result<Vec<layer2_core::sandbox::store::RetiredAgentRuntimePaths>, SandboxStoreError> {
        Ok(cleanup::paths(self, sandbox).await?.agents)
    }

    async fn sandbox_agents(
        &self,
        sandbox_id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        if page.limit == 0 || page.limit > MAX_PAGE_SIZE {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut filter = doc! { "sandbox_id": sandbox_id.as_str() };
        if let Some(cursor) = &page.cursor {
            filter.insert("_id", doc! { "$gt": cursor });
        }
        let mut read = super::mongo_read_telemetry::Read::find(self.agents.name());
        let mut cursor = self
            .agents
            .find(filter)
            .sort(doc! { "_id": 1 })
            .limit(i64::from(page.limit) + 1)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        let mut items = Vec::new();
        while let Some(document) = read
            .next(&mut cursor)
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        {
            items.push(read_agent(&document)?);
        }
        let has_more = items.len() > usize::from(page.limit);
        if has_more {
            items.pop();
        }
        let next = has_more.then(|| {
            items
                .last()
                .expect("a bounded sandbox-agent page has a last item")
                .id
                .0
                .clone()
        });
        read.complete();
        Ok(Page { items, next })
    }

    async fn publish_sandbox_checkpoint(
        &self,
        mutation: SandboxCheckpointMutation,
    ) -> Result<(), SandboxStoreError> {
        super::checkpoints::publish_checkpoint(self, mutation).await
    }

    async fn commit_sandbox_lifecycle(
        &self,
        mutation: SandboxLifecycleMutation,
    ) -> Result<(), SandboxStoreError> {
        super::lifecycle_transaction::commit_lifecycle(self, mutation, None).await
    }

    async fn reopen_refusal_snapshot(
        &self,
        id: &layer2_core::AgentId,
    ) -> Result<Option<AgentMessageSnapshot>, SandboxStoreError> {
        self.agent_message_snapshot(id).await
    }

    async fn park_sandbox_reopen_refusal(
        &self,
        mutation: SandboxLifecycleMutation,
        fence: ReopenRefusalFence,
    ) -> Result<(), SandboxStoreError> {
        super::lifecycle_transaction::commit_lifecycle(self, mutation, Some(fence)).await
    }
}

pub(super) async fn load_sandbox_agents(
    store: &MongoSandboxStore,
    sandbox_id: &SandboxId,
    session: &mut mongodb::ClientSession,
) -> Result<Vec<Agent>, LifecycleLoadError> {
    let mut read = super::mongo_read_telemetry::Read::find(store.agents.name());
    let mut cursor = store
        .agents
        .find(doc! { "sandbox_id": sandbox_id.as_str() })
        .sort(doc! { "_id": 1 })
        .session(&mut *session)
        .await
        .map_err(LifecycleLoadError::Driver)?;
    let mut documents = Vec::new();
    while let Some(document) = read
        .next_in_session(&mut cursor, session)
        .await
        .map_err(LifecycleLoadError::Driver)?
    {
        documents.push(document);
    }
    let agents = documents
        .iter()
        .map(read_agent)
        .collect::<Result<Vec<_>, _>>()
        .map_err(LifecycleLoadError::Domain)?;
    read.complete();
    Ok(agents)
}
