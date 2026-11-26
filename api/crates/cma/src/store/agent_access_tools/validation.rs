use super::*;
use layer2_core::sandbox::store::AgentProvenanceSnapshot;
use layer2_core::sandbox::{Agent, AgentStatus, AgentWorkStep, Sandbox, WorktreeStatus};

pub(super) fn scope(snapshot: &AgentProvenanceSnapshot, runtime: &RuntimeHandle) -> Result<Scope> {
    let lineage = &snapshot.resource.lineage;
    let agent = lineage.agent.as_ref().ok_or(Error::Refused)?;
    let tree = lineage.worktree.as_ref().ok_or(Error::Refused)?;
    let workspace = lineage.workspace.as_ref().ok_or(Error::Refused)?;
    let sandbox = &lineage.sandbox;
    let process = sandbox.runtime.as_ref().ok_or(Error::Refused)?;
    let creation = snapshot
        .creation_provenance(&agent.id)
        .map_err(|_| Error::Refused)?;
    let tree_ready = tree.status == WorktreeStatus::Ready
        || (tree.status == WorktreeStatus::Creating
            && agent.status == AgentStatus::Starting
            && agent.work.lease.is_some()
            && matches!(
                agent.work.step,
                AgentWorkStep::OpenConversation | AgentWorkStep::PublishReady
            ));
    if !sandbox.credential.source.is_managed()
        || !agent.status.is_live()
        || agent.requested_stop_reason.is_some()
        || agent.stopped_at.is_some()
        || agent.deleted_at.is_some()
        || !parent_may_open(sandbox)
        || workspace.status != layer2_core::sandbox::WorkspaceStatus::Ready
        || !tree_ready
        || tree.agent_id.as_ref() != Some(&agent.id)
        || process.runtime_id != runtime.0
        || process.provision_generation.is_none_or(|value| value == 0)
        || !sandbox.credential.delivered
        || sandbox
            .credential
            .current
            .as_ref()
            .is_none_or(|credential| {
                layer2_core::seconds_from_rfc3339(&credential.expires_at).is_none_or(|expiry| {
                    std::time::UNIX_EPOCH + std::time::Duration::from_secs(expiry)
                        <= std::time::SystemTime::now()
                })
            })
        || sandbox.credential.pending.is_some()
    {
        return Err(Error::Refused);
    }
    Ok(Scope {
        creation,
        runtime: process.clone(),
    })
}

pub(super) fn request(
    snapshot: &AgentProvenanceSnapshot,
    request: &AgentOpenRequest,
    fence: u64,
) -> Result<()> {
    request_identity(snapshot, request)?;
    let agent = snapshot
        .resource
        .lineage
        .agent
        .as_ref()
        .ok_or(Error::Refused)?;
    if !valid_fence(agent, fence) {
        return Err(Error::Refused);
    }
    Ok(())
}

/// A retained attachment outlives the lease that published it. Only maintenance
/// may use that exact completed lease; its durable operation is checked by the
/// caller. Any newly scheduled/claimed work or a later fence invalidates it.
pub(super) fn retained_request(
    snapshot: &AgentProvenanceSnapshot,
    request: &AgentOpenRequest,
    fence: u64,
) -> Result<()> {
    request_identity(snapshot, request)?;
    let agent = snapshot
        .resource
        .lineage
        .agent
        .as_ref()
        .ok_or(Error::Refused)?;
    let completed = fence > 0
        && agent.work.lease.is_none()
        && agent.work.step == AgentWorkStep::None
        && fence.checked_add(1) == Some(agent.work.next_fence);
    if !valid_fence(agent, fence) && !completed {
        return Err(Error::Refused);
    }
    Ok(())
}

fn request_identity(snapshot: &AgentProvenanceSnapshot, request: &AgentOpenRequest) -> Result<()> {
    if !crate::store::agent_tool_open::matches(&snapshot.resource.lineage, request) {
        return Err(Error::Refused);
    }
    Ok(())
}

pub(super) fn valid_conversation(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

pub(super) fn new_agent(agent: &Agent) -> bool {
    agent.status == AgentStatus::Starting
        && agent.ready_at.is_none()
        && agent.conversation.conversation_id.is_none()
        && agent.work.step == AgentWorkStep::OpenConversation
}

fn valid_fence(agent: &Agent, fence: u64) -> bool {
    fence > 0
        && agent
            .work
            .lease
            .as_ref()
            .map_or(agent.work.next_fence == fence, |lease| {
                lease.fence == fence
                    && layer2_core::seconds_from_rfc3339(&lease.expires_at).is_some_and(|expiry| {
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(expiry)
                            > std::time::SystemTime::now()
                    })
            })
}

fn parent_may_open(sandbox: &Sandbox) -> bool {
    use layer2_core::sandbox::{
        SandboxImageRotationPhase, SandboxLifecycleOperation, SandboxRevivePhase, SandboxStatus,
    };
    match (sandbox.status, sandbox.lifecycle.operation.as_ref()) {
        (SandboxStatus::Active, _) => true,
        (SandboxStatus::Reviving, Some(SandboxLifecycleOperation::Revive { phase, .. })) => {
            matches!(
                phase,
                SandboxRevivePhase::ConversationReopen
                    | SandboxRevivePhase::QueuedMessageDelivery
                    | SandboxRevivePhase::PublishActive
            )
        }
        (SandboxStatus::Rotating, Some(SandboxLifecycleOperation::ImageRotation { phase, .. })) => {
            matches!(
                phase,
                SandboxImageRotationPhase::ConversationReopen
                    | SandboxImageRotationPhase::QueuedMessageDelivery
                    | SandboxImageRotationPhase::PublishActive
            )
        }
        _ => false,
    }
}
