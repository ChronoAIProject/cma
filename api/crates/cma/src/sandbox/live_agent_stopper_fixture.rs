use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileSnapshot, ThinkingEffort,
};
use layer2_core::sandbox::{
    AgentConversationError, AgentMessageAssembly, AgentOrigin, AgentUpdateOutcome,
    AgentUpdateRecorder, AgentWorkState, ConversationState, Sandbox, SandboxCredentialState,
    SandboxLimits, SandboxOrigin, SandboxSignInState, SandboxStatus, SandboxWorkState,
    SandboxWorkStep, TypedFailure,
};
use layer2_core::{AgentProfileId, RuntimeCapabilities, SandboxProfileId, WorkspaceId, WorktreeId};
use layer3_adapter::{ConversationInputSender, ConversationUpdate};
use tokio::sync::mpsc;

use crate::sandbox::live_agents::LiveAgentTarget;

use super::*;

pub(super) fn sandbox() -> Sandbox {
    let id = SandboxId::generate();
    Sandbox {
        id,
        owner: SubjectId::new("owner"),
        name: "retiring".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId::generate(),
            revision: 1,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: Vec::new(),
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 1,
                max_agents_per_workspace: 8,
                workspace_profiles: Vec::new(),
                software_install_commands: Vec::new(),
                environment_variables: Vec::new(),
            },
        },
        status: SandboxStatus::Retiring,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: Vec::new(),
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: Vec::new(),
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::StopAgents, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 1,
            max_agents_per_workspace: 8,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 3,
    }
}

pub(super) fn live_agent(sandbox: &Sandbox, index: usize) -> Agent {
    Agent {
        id: AgentId(format!("agt_stopper_{index:03}")),
        worktree_id: WorktreeId(format!("wkt_stopper_{index:03}")),
        workspace_id: WorkspaceId(format!("wks_stopper_{index:03}")),
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        creation_key: format!("stopper-{index}"),
        creation_fingerprint: "b".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId(format!("agp_stopper_{index:03}")),
            revision: 1,
            digest: "c".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: Some("test-model".into()),
                thinking_effort: ThinkingEffort::High,
                permission_level: PermissionLevel::Autonomous,
                skills: Vec::new(),
                system_prompt: "Test precisely.".into(),
            },
        },
        capabilities: RuntimeCapabilities::default(),
        status: AgentStatus::Ready,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: Some(format!("conversation-{index}")),
            runtime_generation: Some(3),
            last_seq: 1,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: Vec::new(),
        },
        work: AgentWorkState::new(AgentWorkStep::None, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: Some(NOW.into()),
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 4,
    }
}

pub(super) fn terminal_agent(mut agent: Agent, status: AgentStatus) -> Agent {
    agent.status = status;
    agent.requested_stop_reason = None;
    agent.work.schedule(AgentWorkStep::None, NOW.into());
    match status {
        AgentStatus::Stopped => {
            agent.stopped_at = Some(NOW.into());
            agent.stop_reason = Some(AgentStopReason::Owner);
        }
        AgentStatus::Failed => {
            agent.failure = Some(TypedFailure {
                class: "agent_failed".into(),
                detail: "agent failed".into(),
            });
        }
        AgentStatus::Deleted => {
            agent.stopped_at = Some(NOW.into());
            agent.stop_reason = Some(AgentStopReason::Owner);
            agent.deleted_at = Some(NOW.into());
        }
        _ => unreachable!("terminal fixture requires a terminal status"),
    }
    agent
}

pub(super) fn live_target(agent: &Agent) -> LiveAgentTarget {
    LiveAgentTarget {
        agent_id: agent.id.clone(),
        conversation_id: agent.conversation.conversation_id.clone().unwrap(),
        runtime_generation: agent.conversation.runtime_generation.unwrap(),
    }
}

pub(super) fn attach(
    registry: &Arc<LiveAgentRegistry>,
    agent: &Agent,
    authority_fence: u64,
) -> mpsc::Sender<ConversationUpdate> {
    let input = ConversationInputSender::spawn(2, |_| async { Ok(()) });
    let (updates, receiver) = mpsc::channel(2);
    let attachment = registry
        .attach(
            agent.id.clone(),
            agent.conversation.conversation_id.clone().unwrap(),
            agent.conversation.runtime_generation.unwrap(),
            authority_fence,
            RuntimeCapabilities::default(),
            input,
            receiver,
            Arc::new(NoopRecorder),
        )
        .unwrap();
    registry.publish_and_activate(&attachment).unwrap();
    updates
}

struct NoopRecorder;

#[async_trait::async_trait]
impl AgentUpdateRecorder for NoopRecorder {
    async fn record_update(
        &self,
        _id: &AgentId,
        _runtime_generation: u64,
        _assembly: &mut AgentMessageAssembly,
        _update: ConversationUpdate,
        _appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        Ok(AgentUpdateOutcome {
            appended: Vec::new(),
            turn_closed: false,
        })
    }
}
