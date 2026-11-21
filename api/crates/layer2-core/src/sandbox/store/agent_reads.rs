use async_trait::async_trait;

use crate::ids::SubjectId;
use crate::sandbox::{Agent, AgentListQuery};

use super::{Page, PageRequest, SandboxStoreError};

/// Owner-scoped, storage-side agent filtering with bounded cursor paging.
#[async_trait]
pub trait AgentReadQueryStore: Send + Sync {
    /// Read the immutable capacity prefix from this exact Agent incarnation.
    /// Absence is distinct from unsupported storage or an invalid checkpoint.
    async fn retained_agent_output(
        &self,
        _agent: &Agent,
    ) -> Result<Option<super::AgentMessageCheckpoint>, SandboxStoreError> {
        Err(SandboxStoreError::Unavailable(
            "retained output reads are unavailable".into(),
        ))
    }

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
