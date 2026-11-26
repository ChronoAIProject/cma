//! Validate tool attachment against the original or durably dispatched Agent Open.
use layer2_core::resource_access::ResourceLineage;
use layer2_core::sandbox::{AgentStatus, AgentWorkStep, SandboxStatus, WorktreeStatus};
use layer3_adapter::{AgentOpenRequest, AgentProcessAction};

pub(super) fn matches(lineage: &ResourceLineage, request: &AgentOpenRequest) -> bool {
    let (Some(agent), Some(tree)) = (&lineage.agent, &lineage.worktree) else {
        return false;
    };
    let sandbox = &lineage.sandbox;
    if !request.has_valid_sha256()
        || request.options != crate::sandbox::runtime_options(agent, sandbox, tree)
        || request
            .resume_conversation
            .as_ref()
            .is_some_and(|id| agent.conversation.conversation_id.as_ref() != Some(id))
    {
        return false;
    }
    let incarnation =
        AgentProcessAction::Open.incarnation(agent.id.as_str(), &request.operation_id);
    let Some(pin) = &request.restart_runtime else {
        return incarnation == Some(1)
            && !matches!(
                agent.work.step,
                AgentWorkStep::RestartConversation | AgentWorkStep::RestartTeardown
            );
    };
    if !incarnation.is_some_and(|incarnation| incarnation > 1)
        || request.resume_conversation.is_none()
        || sandbox.status != SandboxStatus::Active
        || sandbox.lifecycle.operation.is_some()
        || tree.status != WorktreeStatus::Ready
        || pin.runtime.sandbox_id != sandbox.id.as_str()
        || sandbox.runtime.as_ref().is_none_or(|runtime| {
            runtime.boot_id.as_ref() != Some(&pin.boot_id)
                || runtime.provision_generation != Some(pin.runtime.generation)
        })
        || sandbox
            .lifecycle
            .home
            .as_ref()
            .and_then(|home| home.attachment())
            .and_then(|home| home.identity().ok())
            .as_ref()
            != Some(&pin.runtime.home_identity)
    {
        return false;
    }
    // Dispatch commits the exact successor Open as cleanup debt before effects.
    // Ready publication consumes that debt and records the completed Open.
    if agent.work.step == AgentWorkStep::RestartConversation {
        return agent.status == AgentStatus::Starting
            && matches!(agent.work.cleanup_debt.as_slice(), [debt]
                if debt.step == AgentWorkStep::OpenConversation
                    && debt.operation_id == request.operation_id
                    && debt.request_sha256 == request.request_sha256);
    }
    agent.work.step != AgentWorkStep::RestartTeardown
        && agent.work.receipts.iter().any(|receipt| {
            receipt.step == AgentWorkStep::OpenConversation
                && receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
        })
}

pub(super) fn published(lineage: &ResourceLineage, request: &AgentOpenRequest) -> bool {
    let Some(agent) = &lineage.agent else {
        return false;
    };
    if matches!(
        agent.work.step,
        AgentWorkStep::RestartConversation | AgentWorkStep::RestartTeardown
    ) {
        return false;
    }
    if request.restart_runtime.is_some() {
        // The restarted connection advances its own generation. The Sandbox
        // provision generation still names the same boot and persistent home.
        return matches!(
            agent.status,
            AgentStatus::Ready | AgentStatus::Working | AgentStatus::WaitingApproval
        ) && matches(lineage, request);
    }
    agent.conversation.runtime_generation
        == lineage
            .sandbox
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.provision_generation)
}
