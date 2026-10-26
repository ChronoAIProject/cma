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
    if !agent.status.is_live()
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
    let lineage = &snapshot.resource.lineage;
    let agent = lineage.agent.as_ref().ok_or(Error::Refused)?;
    let tree = lineage.worktree.as_ref().ok_or(Error::Refused)?;
    let expected = AgentOpenRequest::new(
        format!("agent:{}:open:v1", agent.id),
        crate::sandbox::runtime_options(agent, &lineage.sandbox, tree),
        request.resume_conversation.clone(),
    );
    if !request.has_valid_sha256()
        || request != &expected
        || !valid_fence(agent, fence)
        || request
            .resume_conversation
            .as_deref()
            .is_some_and(|value| agent.conversation.conversation_id.as_deref() != Some(value))
    {
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
