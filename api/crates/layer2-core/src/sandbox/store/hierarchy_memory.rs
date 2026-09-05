use async_trait::async_trait;

use super::*;
use crate::sandbox::{
    HierarchyCounts, HierarchyReadStore, SandboxHierarchyRecord, SandboxInventoryRecord,
    WorkspaceHierarchyRecord,
};

fn count(value: usize) -> u64 {
    value as u64
}

fn counts(state: &State, sandbox_id: &SandboxId) -> HierarchyCounts {
    HierarchyCounts {
        workspaces: count(
            state
                .workspaces
                .values()
                .filter(|value| &value.sandbox_id == sandbox_id)
                .count(),
        ),
        worktrees: count(
            state
                .worktrees
                .values()
                .filter(|value| &value.sandbox_id == sandbox_id)
                .count(),
        ),
        agents: count(
            state
                .agents
                .values()
                .filter(|value| &value.sandbox_id == sandbox_id)
                .count(),
        ),
    }
}

#[async_trait]
impl HierarchyReadStore for InMemorySandboxStore {
    async fn list_owner_sandbox_inventory(
        &self,
        owner: &SubjectId,
        status: Option<SandboxStatus>,
        request: &PageRequest,
    ) -> Result<Page<SandboxInventoryRecord>, SandboxStoreError> {
        let state = self.lock();
        let values = state
            .sandboxes
            .values()
            .filter(|sandbox| {
                &sandbox.owner == owner && status.is_none_or(|expected| sandbox.status == expected)
            })
            .map(|sandbox| {
                (
                    sandbox.id.to_string(),
                    SandboxInventoryRecord {
                        sandbox: sandbox.clone(),
                        counts: counts(&state, &sandbox.id),
                    },
                )
            })
            .collect();
        page(values, request)
    }

    async fn read_sandbox_hierarchy(
        &self,
        id: &SandboxId,
    ) -> Result<Option<SandboxHierarchyRecord>, SandboxStoreError> {
        let state = self.lock();
        let Some(sandbox) = state.sandboxes.get(id).cloned() else {
            return Ok(None);
        };
        Ok(Some(SandboxHierarchyRecord {
            sandbox,
            workspaces: state
                .workspaces
                .values()
                .filter(|value| &value.sandbox_id == id)
                .cloned()
                .collect(),
            worktrees: state
                .worktrees
                .values()
                .filter(|value| &value.sandbox_id == id)
                .cloned()
                .collect(),
            agents: state
                .agents
                .values()
                .filter(|value| &value.sandbox_id == id)
                .cloned()
                .collect(),
        }))
    }

    async fn read_workspace_hierarchy(
        &self,
        id: &WorkspaceId,
    ) -> Result<Option<WorkspaceHierarchyRecord>, SandboxStoreError> {
        let state = self.lock();
        let Some(workspace) = state.workspaces.get(id).cloned() else {
            return Ok(None);
        };
        Ok(Some(WorkspaceHierarchyRecord {
            workspace,
            worktrees: state
                .worktrees
                .values()
                .filter(|value| &value.workspace_id == id)
                .cloned()
                .collect(),
            agents: state
                .agents
                .values()
                .filter(|value| &value.workspace_id == id)
                .cloned()
                .collect(),
        }))
    }

    async fn read_hierarchy_worktree(
        &self,
        id: &WorktreeId,
    ) -> Result<Option<Worktree>, SandboxStoreError> {
        Ok(self.lock().worktrees.get(id).cloned())
    }
}
