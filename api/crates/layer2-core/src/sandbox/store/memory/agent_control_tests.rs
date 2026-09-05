use std::sync::Arc;

use crate::ids::{AgentId, SandboxId, SubjectId, WorkspaceId, WorktreeId};
use crate::sandbox::store::{
    AgentAdmissionStore, AgentControlResult, AgentControlStore, AgentControlStoreError,
    AgentControlTarget, AgentDeleteRequest, AgentLogStore, AgentStopRequest, AgentStore,
    AgentWorkMutation, AgentWorkerStore, SandboxEventStore, SandboxStoreError, VersionedWorktree,
};
use crate::sandbox::{
    Agent, AgentLogKind, AgentStatus, AgentStopReason, AgentWorkStep, SandboxEventKind,
    TypedFailure, WorktreeStatus,
};

use super::agent_admission_tests::{admission, created, ready_store, NOW};
use super::InMemorySandboxStore;

mod delete;
mod integrity;
mod stop;

const MINUTE: &str = "2026-09-05T00:01:00Z";
const TWO_MINUTES: &str = "2026-09-05T00:02:00Z";

fn stop_request(agent: &Agent, reason: AgentStopReason) -> AgentStopRequest {
    AgentStopRequest {
        target: AgentControlTarget::from(agent),
        reason,
        requested_at: MINUTE.into(),
    }
}

fn delete_request(agent: &Agent) -> AgentDeleteRequest {
    AgentDeleteRequest {
        target: AgentControlTarget::from(agent),
        requested_at: MINUTE.into(),
    }
}

fn replace_agent(store: &InMemorySandboxStore, agent: Agent) {
    store.lock().agents.insert(agent.id.clone(), agent);
}

fn replace_worktree_status(store: &InMemorySandboxStore, agent: &Agent, status: WorktreeStatus) {
    store
        .lock()
        .worktrees
        .get_mut(&agent.worktree_id)
        .unwrap()
        .status = status;
}

fn terminal_agent(mut agent: Agent, status: AgentStatus) -> Agent {
    agent.status = status;
    agent.work.schedule(AgentWorkStep::None, NOW.into());
    agent.requested_stop_reason = None;
    agent.stop_reason = None;
    agent.stopped_at = None;
    agent.deleted_at = None;
    agent.failure = None;
    match status {
        AgentStatus::Stopped => {
            agent.conversation.conversation_id = Some("conversation-control".into());
            agent.conversation.runtime_generation = Some(1);
            agent.stopped_at = Some(NOW.into());
            agent.stop_reason = Some(AgentStopReason::Owner);
        }
        AgentStatus::Failed => {
            agent.failure = Some(TypedFailure {
                class: "startup_failed".into(),
                detail: "startup failed".into(),
            });
        }
        AgentStatus::Deleted => {
            agent.conversation.conversation_id = Some("conversation-control".into());
            agent.conversation.runtime_generation = Some(1);
            agent.stopped_at = Some(NOW.into());
            agent.stop_reason = Some(AgentStopReason::Owner);
            agent.deleted_at = Some(NOW.into());
        }
        _ => {}
    }
    agent
}

fn all_work_steps() -> [AgentWorkStep; 14] {
    [
        AgentWorkStep::AddWorktree,
        AgentWorkStep::MaterializeHome,
        AgentWorkStep::OpenConversation,
        AgentWorkStep::PublishReady,
        AgentWorkStep::PublishConnection,
        AgentWorkStep::DispatchFirstMessage,
        AgentWorkStep::FenceInteraction,
        AgentWorkStep::CloseProcess,
        AgentWorkStep::PublishStopped,
        AgentWorkStep::EnsureProcessClosed,
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
        AgentWorkStep::PublishDeleted,
        AgentWorkStep::None,
    ]
}
