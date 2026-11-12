use async_trait::async_trait;

use super::*;
use crate::sandbox::{
    AgentAdminSummary, AgentStatus, SandboxAdminQuery, SandboxAdminStore, SandboxAdminSummary,
    SandboxAdminView, WorkspaceAdminSummary,
};

fn runtime_matches(sandbox: &Sandbox, expected: &str) -> bool {
    sandbox
        .runtime
        .as_ref()
        .is_some_and(|runtime| runtime.runtime_version == expected)
}

fn live_agent_count(state: &State, sandbox_id: &SandboxId) -> u64 {
    state
        .agents
        .values()
        .filter(|agent| &agent.sandbox_id == sandbox_id && agent.status != AgentStatus::Deleted)
        .count() as u64
}

#[async_trait]
impl SandboxAdminStore for InMemorySandboxStore {
    async fn list_admin_sandboxes(
        &self,
        query: &SandboxAdminQuery,
    ) -> Result<Page<SandboxAdminSummary>, SandboxStoreError> {
        let state = self.lock();
        let values = state
            .sandboxes
            .values()
            .filter(|sandbox| {
                query
                    .owner
                    .as_ref()
                    .is_none_or(|owner| &sandbox.owner == owner)
                    && query.status.is_none_or(|status| sandbox.status == status)
                    && query
                        .always_on
                        .is_none_or(|value| (sandbox.profile.always_on == Some(true)) == value)
                    && query
                        .runtime_version
                        .as_deref()
                        .is_none_or(|version| runtime_matches(sandbox, version))
            })
            .map(|sandbox| {
                (
                    sandbox.id.to_string(),
                    SandboxAdminSummary::from_sandbox(
                        sandbox,
                        live_agent_count(&state, &sandbox.id),
                    ),
                )
            })
            .collect();
        page(values, &query.page)
    }

    async fn read_admin_sandbox(
        &self,
        id: &SandboxId,
    ) -> Result<Option<SandboxAdminView>, SandboxStoreError> {
        let state = self.lock();
        let Some(sandbox) = state.sandboxes.get(id).cloned() else {
            return Ok(None);
        };
        let mut workspaces: Vec<_> = state
            .workspaces
            .values()
            .filter(|workspace| &workspace.sandbox_id == id)
            .map(WorkspaceAdminSummary::from)
            .collect();
        workspaces.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        let mut agents: Vec<_> = state
            .agents
            .values()
            .filter(|agent| &agent.sandbox_id == id)
            .map(AgentAdminSummary::from)
            .collect();
        agents.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        Ok(Some(SandboxAdminView {
            sandbox,
            workspaces,
            agents,
        }))
    }

    async fn read_admin_sandbox_events(
        &self,
        id: &SandboxId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<SandboxEvent>, SandboxStoreError> {
        if !self.lock().sandboxes.contains_key(id) {
            return Err(SandboxStoreError::NotFound);
        }
        <Self as SandboxEventStore>::sandbox_events(self, id, after, limit).await
    }
}
