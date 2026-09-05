use async_trait::async_trait;

use crate::ids::SubjectId;
use crate::sandbox::{Agent, AgentListQuery};

use super::{Page, PageRequest, SandboxStoreError};

/// Owner-scoped, storage-side agent filtering with bounded cursor paging.
#[async_trait]
pub trait AgentReadQueryStore: Send + Sync {
    async fn list_owned_agents(
        &self,
        owner: &SubjectId,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError>;
}

impl AgentListQuery {
    pub(crate) fn matches(&self, agent: &Agent) -> bool {
        self.sandbox_id
            .as_ref()
            .is_none_or(|id| id == &agent.sandbox_id)
            && self
                .workspace_id
                .as_ref()
                .is_none_or(|id| id == &agent.workspace_id)
            && self.status.is_none_or(|status| status == agent.status)
    }
}
