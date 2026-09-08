use std::collections::BTreeSet;
use std::time::SystemTime;

use layer2_core::rfc3339_at;
use layer2_core::sandbox::store::{
    AgentControlStoreError, AgentControlTarget, AgentDeleteRequest, AgentStopRequest, PageRequest,
};
use layer2_core::sandbox::{
    AgentStatus, AgentStopReason, AgentWorkStep, Workspace, WorkspaceStatus,
};

use super::support::WorkerFailure;
use super::WorkspaceWorker;

impl WorkspaceWorker {
    pub(super) async fn remove_child_agents(
        &self,
        workspace: &Workspace,
        now: SystemTime,
    ) -> Result<bool, WorkerFailure> {
        let current = self
            .dependencies
            .store
            .get_workspace(&workspace.id)
            .await
            .map_err(|_| WorkerFailure::retryable("workspace removal authority is unavailable"))?;
        if current.as_ref() != Some(workspace) || workspace.status != WorkspaceStatus::Removing {
            return Err(WorkerFailure::retryable(
                "workspace removal authority changed",
            ));
        }
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        let mut complete = true;
        loop {
            let page = self
                .dependencies
                .store
                .list_agents(&workspace.owner, &PageRequest { limit: 200, cursor })
                .await
                .map_err(|_| WorkerFailure::retryable("workspace agents could not be loaded"))?;
            for agent in page
                .items
                .iter()
                .filter(|agent| agent.workspace_id == workspace.id)
            {
                if agent.owner != workspace.owner || agent.sandbox_id != workspace.sandbox_id {
                    return Err(WorkerFailure::terminal(
                        "workspace_child_lineage_invalid",
                        "workspace agents do not belong to the same environment",
                    ));
                }
                if agent.status == AgentStatus::Deleted && agent.work.step == AgentWorkStep::None {
                    continue;
                }
                complete = false;
                if agent.status.is_live() {
                    // Preserve earlier owner or operator stop intent while its worker finishes.
                    if agent.requested_stop_reason.is_none() {
                        self.dependencies
                            .store
                            .schedule_agent_stop(AgentStopRequest {
                                target: AgentControlTarget::from(agent),
                                reason: AgentStopReason::Owner,
                                requested_at: rfc3339_at(now),
                            })
                            .await
                            .map_err(control_failure)?;
                    }
                } else if matches!(agent.status, AgentStatus::Stopped | AgentStatus::Failed)
                    && agent.work.step == AgentWorkStep::None
                {
                    self.dependencies
                        .store
                        .schedule_agent_delete(AgentDeleteRequest {
                            target: AgentControlTarget::from(agent),
                            requested_at: rfc3339_at(now),
                        })
                        .await
                        .map_err(control_failure)?;
                }
            }
            cursor = page.next;
            match cursor.as_ref() {
                None => return Ok(complete),
                Some(next) if seen.insert(next.clone()) => {}
                Some(_) => {
                    return Err(WorkerFailure::terminal(
                        "workspace_agent_pagination_invalid",
                        "workspace agent inventory did not advance",
                    ))
                }
            }
        }
    }
}

fn control_failure(error: AgentControlStoreError) -> WorkerFailure {
    match error {
        AgentControlStoreError::Corrupt(_) => WorkerFailure::terminal(
            "workspace_child_invalid",
            "a workspace agent has inconsistent cleanup records",
        ),
        _ => {
            WorkerFailure::retryable("workspace agent cleanup is waiting for its current operation")
        }
    }
}
