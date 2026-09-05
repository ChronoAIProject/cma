//! Atomic Mongo scheduling for owner-authorized agent stop and deletion.

use layer2_core::sandbox::store::{
    AgentControlResult, AgentControlStore, AgentControlStoreError, AgentControlTarget,
    AgentDeleteRequest, AgentStopRequest, SandboxStoreError,
};
use layer2_core::sandbox::{Agent, AgentStatus, AgentWorkStep};
use mongodb::bson::doc;

use super::agents::{agent_replacement_fields, read_agent};
use super::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::sandboxes::read_sandbox;
use super::workspaces::read_workspace;
use super::worktrees::read_worktree;

const CONTROL_ATTEMPTS: usize = 8;

#[async_trait::async_trait]
impl AgentControlStore for MongoSandboxStore {
    async fn schedule_agent_stop(
        &self,
        request: AgentStopRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        for _ in 0..CONTROL_ATTEMPTS {
            let current = control_agent(self, &request.target).await?;
            match plan_stop(current.clone(), &request)? {
                existing @ AgentControlResult::Existing(_) => return Ok(existing),
                AgentControlResult::Scheduled(next) => {
                    if replace_agent(self, &current, &next).await? {
                        return Ok(AgentControlResult::Scheduled(next));
                    }
                }
            }
            tokio::task::yield_now().await;
        }
        Err(AgentControlStoreError::Conflict)
    }

    async fn schedule_agent_delete(
        &self,
        request: AgentDeleteRequest,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        for _ in 0..CONTROL_ATTEMPTS {
            let current = control_agent(self, &request.target).await?;
            match plan_delete(current.clone(), &request)? {
                existing @ AgentControlResult::Existing(_) => return Ok(existing),
                AgentControlResult::Scheduled(next) => {
                    if replace_agent(self, &current, &next).await? {
                        return Ok(AgentControlResult::Scheduled(next));
                    }
                }
            }
            tokio::task::yield_now().await;
        }
        Err(AgentControlStoreError::Conflict)
    }
}

async fn control_agent(
    store: &MongoSandboxStore,
    target: &AgentControlTarget,
) -> Result<Agent, AgentControlStoreError> {
    let agent = store
        .agents
        .find_one(doc! { "_id": target.agent_id.as_str() })
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(read_agent)
        .transpose()
        .map_err(store_error)?
        .ok_or(AgentControlStoreError::NotFound)?;
    if agent.id != target.agent_id {
        return Err(AgentControlStoreError::Corrupt(
            "agent storage identity does not match its key".into(),
        ));
    }
    if agent.owner != target.owner {
        return Err(AgentControlStoreError::NotFound);
    }
    if agent.revision == 0 || agent.work.next_fence == 0 {
        return Err(AgentControlStoreError::Corrupt(
            "agent revision or work fence is invalid".into(),
        ));
    }
    if agent.worktree_id != target.worktree_id
        || agent.workspace_id != target.workspace_id
        || agent.sandbox_id != target.sandbox_id
    {
        return Err(AgentControlStoreError::Conflict);
    }
    validate_stored_lineage(store, &agent).await?;
    Ok(agent)
}

async fn validate_stored_lineage(
    store: &MongoSandboxStore,
    agent: &Agent,
) -> Result<(), AgentControlStoreError> {
    let worktree = store
        .worktrees
        .find_one(doc! { "_id": agent.worktree_id.as_str() })
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(read_worktree)
        .transpose()
        .map_err(store_error)?
        .ok_or_else(|| AgentControlStoreError::Corrupt("agent worktree is missing".into()))?;
    let workspace = store
        .workspaces
        .find_one(doc! { "_id": agent.workspace_id.as_str() })
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(read_workspace)
        .transpose()
        .map_err(store_error)?
        .ok_or_else(|| AgentControlStoreError::Corrupt("agent workspace is missing".into()))?;
    let sandbox = store
        .sandboxes
        .find_one(doc! { "_id": agent.sandbox_id.as_str() })
        .await
        .map_err(unavailable)?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(store_error)?
        .ok_or_else(|| AgentControlStoreError::Corrupt("agent sandbox is missing".into()))?;
    if worktree.id != agent.worktree_id
        || worktree.agent_id.as_ref() != Some(&agent.id)
        || worktree.workspace_id != agent.workspace_id
        || worktree.sandbox_id != agent.sandbox_id
        || worktree.owner != agent.owner
        || workspace.id != agent.workspace_id
        || workspace.sandbox_id != agent.sandbox_id
        || workspace.owner != agent.owner
        || sandbox.id != agent.sandbox_id
        || sandbox.owner != agent.owner
    {
        return Err(AgentControlStoreError::Corrupt(
            "agent lineage does not match its parents".into(),
        ));
    }
    Ok(())
}

pub(super) fn plan_stop(
    current: Agent,
    request: &AgentStopRequest,
) -> Result<AgentControlResult, AgentControlStoreError> {
    if !current.status.is_live() {
        require_exact_target(&current, &request.target)?;
        return Err(AgentControlStoreError::StatusConflict {
            status: current.status,
        });
    }
    if current.stop_reason.is_some() || current.stopped_at.is_some() || current.deleted_at.is_some()
    {
        return Err(AgentControlStoreError::Corrupt(
            "live agent contains terminal stop data".into(),
        ));
    }
    if let Some(existing) = current.requested_stop_reason.as_ref() {
        if existing != &request.reason {
            return Err(AgentControlStoreError::StopReasonConflict);
        }
        return if is_stop_work(current.work.step) {
            Ok(AgentControlResult::Existing(current))
        } else {
            Err(AgentControlStoreError::WorkConflict {
                step: current.work.step,
            })
        };
    }
    require_exact_target(&current, &request.target)?;
    if !can_supersede_with_stop(current.work.step) {
        return Err(AgentControlStoreError::WorkConflict {
            step: current.work.step,
        });
    }
    let mut next = current;
    next.request_stop(request.reason.clone(), request.requested_at.clone())
        .map_err(|_| AgentControlStoreError::Conflict)?;
    Ok(AgentControlResult::Scheduled(next))
}

pub(super) fn plan_delete(
    current: Agent,
    request: &AgentDeleteRequest,
) -> Result<AgentControlResult, AgentControlStoreError> {
    if current.status == AgentStatus::Deleted {
        return if current.work.step == AgentWorkStep::None || is_delete_work(current.work.step) {
            Ok(AgentControlResult::Existing(current))
        } else {
            Err(AgentControlStoreError::WorkConflict {
                step: current.work.step,
            })
        };
    }
    if matches!(current.status, AgentStatus::Stopped | AgentStatus::Failed)
        && current.requested_stop_reason.is_some()
    {
        return Err(AgentControlStoreError::Corrupt(
            "terminal agent still contains a requested stop reason".into(),
        ));
    }
    if matches!(current.status, AgentStatus::Stopped | AgentStatus::Failed)
        && is_delete_work(current.work.step)
    {
        return Ok(AgentControlResult::Existing(current));
    }
    require_exact_target(&current, &request.target)?;
    if !matches!(current.status, AgentStatus::Stopped | AgentStatus::Failed) {
        return Err(AgentControlStoreError::StatusConflict {
            status: current.status,
        });
    }
    if current.work.step != AgentWorkStep::None {
        return Err(AgentControlStoreError::WorkConflict {
            step: current.work.step,
        });
    }
    let mut next = current;
    next.request_delete(request.requested_at.clone())
        .map_err(|_| AgentControlStoreError::Conflict)?;
    Ok(AgentControlResult::Scheduled(next))
}

async fn replace_agent(
    store: &MongoSandboxStore,
    current: &Agent,
    next: &Agent,
) -> Result<bool, AgentControlStoreError> {
    let revision =
        signed(current.revision, "agent revision").map_err(|_| AgentControlStoreError::Conflict)?;
    let last_seq = signed(current.conversation.last_seq, "agent log sequence")
        .map_err(AgentControlStoreError::Corrupt)?;
    let result = store
        .agents
        .update_one(
            doc! {
                "_id": current.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": revision,
                "last_seq": last_seq,
                "record.conversation.last_seq": last_seq,
            },
            doc! { "$set": agent_replacement_fields(next).map_err(store_error)? },
        )
        .await
        .map_err(unavailable)?;
    Ok(result.matched_count == 1)
}

fn require_exact_target(
    agent: &Agent,
    target: &AgentControlTarget,
) -> Result<(), AgentControlStoreError> {
    if agent.status != target.expected_status || agent.revision != target.expected_revision {
        return Err(AgentControlStoreError::Conflict);
    }
    Ok(())
}

fn can_supersede_with_stop(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::AddWorktree
            | AgentWorkStep::MaterializeHome
            | AgentWorkStep::OpenConversation
            | AgentWorkStep::PublishReady
            | AgentWorkStep::DispatchFirstMessage
            | AgentWorkStep::None
    )
}

fn is_stop_work(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::FenceInteraction
            | AgentWorkStep::CloseProcess
            | AgentWorkStep::PublishStopped
    )
}

fn is_delete_work(step: AgentWorkStep) -> bool {
    matches!(
        step,
        AgentWorkStep::EnsureProcessClosed
            | AgentWorkStep::RemoveHome
            | AgentWorkStep::RemoveWorktree
            | AgentWorkStep::PublishDeleted
    )
}

fn store_error(error: SandboxStoreError) -> AgentControlStoreError {
    match error {
        SandboxStoreError::NotFound => AgentControlStoreError::NotFound,
        SandboxStoreError::Corrupt(detail) => AgentControlStoreError::Corrupt(detail),
        other => AgentControlStoreError::Unavailable(other.to_string()),
    }
}

fn unavailable(error: mongodb::error::Error) -> AgentControlStoreError {
    AgentControlStoreError::Unavailable(error.to_string())
}
