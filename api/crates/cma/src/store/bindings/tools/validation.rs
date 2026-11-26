use super::*;
use layer2_core::sandbox::binding::{BindingConversationState, BindingState, BindingWork};
use layer2_core::sandbox::store::AgentProvenanceSnapshot;
use layer2_core::sandbox::{Agent, AgentCreationOrigin, RuntimeBinding};
use layer3_adapter::ToolProxyRequest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scope {
    pub agent_id: layer2_core::AgentId,
    pub(super) agent_created_at: String,
    pub(super) creation_fingerprint: String,
    pub(super) binding_id: String,
    pub(super) binding_generation: u64,
    pub(super) conversation_id: String,
    pub runtime: RuntimeBinding,
    pub(super) request_sha256: String,
    pub(super) endpoint: String,
    // The launch digest can repeat on later Opens of the same conversation.
    // Existing scope IDs omit the field for the original Open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) numbered_open: Option<String>,
}

impl Scope {
    pub(super) fn id(&self) -> layer3_adapter::Result<String> {
        layer2_core::profiles::content_digest(self).map_err(unavailable)
    }

    pub(super) fn matches_open(&self, request: &AgentOpenRequest) -> bool {
        self.request_sha256 == request.request_sha256
            && self.numbered_open.as_deref()
                == request
                    .restart_runtime
                    .as_ref()
                    .map(|_| request.operation_id.as_str())
    }
}

pub(super) fn validate(
    snapshot: &AgentProvenanceSnapshot,
    binding: &ProviderBinding,
    conversation: &BindingConversation,
    runtime: &RuntimeHandle,
    request: &AgentOpenRequest,
    fence: u64,
) -> layer3_adapter::Result<(Scope, ToolProxyRequest)> {
    let (mut scope, proxy) = identity(snapshot, binding, conversation, runtime)?;
    let lineage = &snapshot.resource.lineage;
    let agent = lineage.agent.as_ref().ok_or_else(refused)?;
    if !crate::store::agent_tool_open::matches(lineage, request) || !valid_fence(agent, fence) {
        return Err(refused());
    }
    scope.request_sha256 = request.request_sha256.clone();
    scope.numbered_open = request
        .restart_runtime
        .as_ref()
        .map(|_| request.operation_id.clone());
    Ok((scope, proxy))
}

/// Current immutable provider lineage, independent of mutable display settings
/// and the worker lease which is released after successful installation.
pub(super) fn identity(
    snapshot: &AgentProvenanceSnapshot,
    binding: &ProviderBinding,
    conversation: &BindingConversation,
    runtime: &RuntimeHandle,
) -> layer3_adapter::Result<(Scope, ToolProxyRequest)> {
    let creation = snapshot
        .creation_provenance(&conversation.agent_id)
        .map_err(unavailable)?;
    let lineage = &snapshot.resource.lineage;
    let agent = lineage.agent.as_ref().ok_or_else(refused)?;
    let sandbox = &lineage.sandbox;
    let workspace = lineage.workspace.as_ref().ok_or_else(refused)?;
    let tree = lineage.worktree.as_ref().ok_or_else(refused)?;
    let environment = binding.profile.environment.as_ref().ok_or_else(refused)?;
    let process = sandbox.runtime.as_ref().ok_or_else(refused)?;
    let endpoint = binding.tool_endpoint.as_ref().ok_or_else(refused)?;
    let credential = sandbox.credential.current.as_ref().ok_or_else(refused)?;
    // Recompute the immutable admission identity, including profile pins. The
    // current display title and later conversation input are not creation data.
    let admission = layer2_core::sandbox::store::AgentAdmission {
        binding: None,
        created_by: creation.created_by.clone(),
        owner: binding.owner.clone(),
        workspace_id: binding.workspace_id.clone(),
        agent_id: conversation.agent_id.clone(),
        worktree_id: conversation.worktree_id.clone(),
        profile: agent.profile.clone(),
        capabilities: agent.capabilities.clone(),
        title: None,
        first_message: None,
        origin: layer2_core::sandbox::AgentOrigin::Manual,
        creation_key: conversation.creation_key().map_err(unavailable)?,
        event_at: agent.created_at.clone(),
    };
    if binding.state != BindingState::Ready
        || binding.work != BindingWork::Idle
        || !parent_may_open(sandbox)
        || workspace.status != layer2_core::sandbox::WorkspaceStatus::Ready
        || !worktree_may_open(agent, tree)
        || !matches!(
            conversation.state,
            BindingConversationState::Provisioning | BindingConversationState::Ready
        )
        || !matches!(creation.creation_origin, AgentCreationOrigin::Manual)
        || creation.created_by
            != Some(layer2_core::resource_access::ResourceActor::Api {
                subject: binding.manager.clone(),
                delegation_id: None,
            })
        || conversation.binding_id != binding.binding_id
        || conversation.generation != binding.generation
        || conversation.owner != binding.owner
        || agent.owner != binding.owner
        || sandbox.owner != binding.owner
        || conversation.agent_id != agent.id
        || conversation.sandbox_id != binding.sandbox_id
        || sandbox.id != binding.sandbox_id
        || conversation.workspace_id != binding.workspace_id
        || workspace.id != binding.workspace_id
        || conversation.worktree_id != agent.worktree_id
        || tree.id != conversation.worktree_id
        || agent.creation_key != conversation.creation_key().map_err(unavailable)?
        || agent.creation_fingerprint != admission.fingerprint().map_err(unavailable)?
        || agent.profile.content != binding.profile.agent
        || sandbox.profile.content != environment.sandbox
        || workspace.profile.content != environment.workspace
        || sandbox.credential.source != binding.credential_source()
        || !sandbox.credential.delivered
        || sandbox.credential.pending.is_some()
        || credential.provider_key_id != format!("provided-{}", binding.key_sha256)
        || process.runtime_id != runtime.0
        || process
            .provision_generation
            .is_none_or(|generation| generation == 0)
        || agent.requested_stop_reason.is_some()
        || !agent.status.is_live()
    {
        return Err(refused());
    }
    let scope = Scope {
        agent_id: agent.id.clone(),
        agent_created_at: agent.created_at.clone(),
        creation_fingerprint: creation.creation_fingerprint,
        binding_id: binding.binding_id.clone(),
        binding_generation: binding.generation,
        conversation_id: conversation.conversation_id.clone(),
        runtime: process.clone(),
        request_sha256: String::new(),
        endpoint: endpoint.clone(),
        numbered_open: None,
    };
    let proxy = ToolProxyRequest {
        sandbox_id: sandbox.id.to_string(),
        generation: process.provision_generation.ok_or_else(refused)?,
        endpoint: endpoint.clone(),
        credential_generation: credential.generation,
    };
    proxy.validate()?;
    Ok((scope, proxy))
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

fn worktree_may_open(agent: &Agent, tree: &layer2_core::sandbox::Worktree) -> bool {
    use layer2_core::sandbox::{AgentStatus, AgentWorkStep, WorktreeStatus};
    match tree.status {
        WorktreeStatus::Ready => true,
        // Startup publishes the Worktree ready only after opening the Agent.
        // Recovery may reconnect that open before completing ready publication.
        WorktreeStatus::Creating => {
            agent.status == AgentStatus::Starting
                && agent.work.lease.is_some()
                && matches!(
                    agent.work.step,
                    AgentWorkStep::OpenConversation | AgentWorkStep::PublishReady
                )
        }
        _ => false,
    }
}

fn parent_may_open(sandbox: &layer2_core::sandbox::Sandbox) -> bool {
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
