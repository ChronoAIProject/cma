use layer2_core::sandbox::{Agent, AgentWorkStep};
use layer3_adapter::{WorktreeReceipt, WorktreeRequest};

use super::support::{AgentContext, WorkerFailure};
use crate::sandbox::live_agents::{LiveAgentRegistryError, LiveAgentTarget};

pub(super) fn live_target(agent: &Agent) -> Result<LiveAgentTarget, WorkerFailure> {
    match (
        agent.conversation.conversation_id.as_ref(),
        agent.conversation.runtime_generation,
    ) {
        (Some(conversation_id), Some(runtime_generation))
            if !conversation_id.trim().is_empty() && runtime_generation > 0 =>
        {
            Ok(LiveAgentTarget {
                agent_id: agent.id.clone(),
                conversation_id: conversation_id.clone(),
                runtime_generation,
            })
        }
        _ => Err(WorkerFailure::terminal(
            "agent_conversation_state_invalid",
            "agent conversation identity is incomplete",
        )),
    }
}

pub(super) fn valid_conversation_id(id: Option<String>) -> Result<String, WorkerFailure> {
    id.filter(|value| !value.trim().is_empty() && value.len() <= 512)
        .ok_or_else(|| {
            WorkerFailure::terminal(
                "agent_conversation_not_ready",
                "runtime did not return a ready conversation identity",
            )
        })
}

pub(super) fn valid_worktree_receipt(
    context: &AgentContext,
    request: &WorktreeRequest,
    receipt: &WorktreeReceipt,
) -> bool {
    receipt.operation_id == request.operation_id
        && receipt.request_sha256 == request.request_sha256
        && receipt.workspace == request.workspace
        && receipt.name == request.name
        && receipt.path == context.worktree.path
        && receipt.branch == context.worktree.branch
        && receipt.base_commit == context.worktree.base_commit
}

pub(super) fn registry_failure(error: LiveAgentRegistryError) -> WorkerFailure {
    match error {
        LiveAgentRegistryError::StaleGeneration | LiveAgentRegistryError::NotAttached => {
            WorkerFailure::retryable("agent_runtime_attachment_conflict", error.to_string())
        }
        LiveAgentRegistryError::ConversationMismatch => {
            WorkerFailure::terminal("agent_conversation_identity_mismatch", error.to_string())
        }
    }
}

pub(super) fn has_startup_evidence(agent: &Agent, step: AgentWorkStep) -> bool {
    agent
        .work
        .receipts
        .iter()
        .any(|receipt| receipt.step == step)
        || agent.work.cleanup_debt.iter().any(|debt| debt.step == step)
}
