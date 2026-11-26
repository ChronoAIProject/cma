use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

use crate::ids::{
    AgentId, AgentProfileId, SandboxId, SandboxProfileId, SubjectId, WorkspaceId, WorktreeId,
};
use crate::profiles::{
    AgentProfileContent, AgentProfileSnapshot, AgentProvider, ModelAccess, PermissionLevel,
    SandboxProfileContent, SandboxProfileSnapshot, ThinkingEffort,
};

use super::*;

pub(super) const NOW: &str = "2026-09-05T00:30:01Z";
pub(super) const IDLE_AT: &str = "2026-09-05T00:15:00Z";

pub(super) fn sandbox() -> Sandbox {
    Sandbox {
        id: SandboxId("sbx_lifecycle".into()),
        owner: SubjectId("owner".into()),
        name: "lifecycle".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_lifecycle".into()),
            revision: 3,
            digest: "a".repeat(64),
            always_on: None,
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![],
                agent_provider: AgentProvider::default(),
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![],
                software_install_commands: vec!["install-once".into()],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Active,
        failure: None,
        runtime: Some(RuntimeBinding {
            boot_id: None,
            runtime_id: "runtime-old".into(),
            runtime_version: "image-v1".into(),
            provision_generation: None,
            bound_at: "2026-09-05T00:00:00Z".into(),
        }),
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: Vec::new(),
            service_scope: None,
            reaches: vec![],
            key_generation: 1,
            delivered: true,
            current: Some(SandboxCredentialAuthority {
                generation: 1,
                provider_key_id: "provider-key".into(),
                provider_state_version: 1,
                issued_at: "2026-09-05T00:00:00Z".into(),
                expires_at: "2026-09-06T00:00:00Z".into(),
                grant_fingerprint: "b".repeat(64),
            }),
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::None, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        lifecycle: Default::default(),
        last_activity_at: IDLE_AT.into(),
        created_at: "2026-09-05T00:00:00Z".into(),
        updated_at: IDLE_AT.into(),
        retired_at: None,
        purge_after: None,
        revision: 7,
    }
}

pub(super) fn agent(id: &str, conversation_id: &str, seq: u64) -> Agent {
    Agent {
        id: AgentId(id.into()),
        worktree_id: WorktreeId(format!("wkt_{id}")),
        workspace_id: WorkspaceId(format!("wks_{id}")),
        sandbox_id: SandboxId("sbx_lifecycle".into()),
        owner: SubjectId("owner".into()),
        creation_key: format!("create-{id}"),
        creation_fingerprint: "c".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_lifecycle".into()),
            revision: 2,
            digest: "d".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: None,
                thinking_effort: ThinkingEffort::Medium,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        },
        capabilities: crate::RuntimeCapabilities::new([crate::RuntimeCapability::Conversation]),
        status: AgentStatus::Ready,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: Some(conversation_id.into()),
            runtime_generation: Some(4),
            last_seq: seq,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::None, IDLE_AT.into()),
        failure: None,
        title: Some("Private title".into()),
        created_at: "2026-09-05T00:00:00Z".into(),
        updated_at: IDLE_AT.into(),
        ready_at: Some("2026-09-05T00:00:01Z".into()),
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 5,
    }
}

pub(super) fn log(agent_id: &str, seq: u64, appended_at: &str) -> AgentLogEntry {
    AgentLogEntry {
        agent_id: AgentId(agent_id.into()),
        seq,
        appended_at: appended_at.into(),
        kind: AgentLogKind::AgentMessage {
            content: "private transcript content".into(),
        },
    }
}

pub(super) fn checkpoint(epoch: u64, sealed_at: &str) -> SandboxCheckpointRef {
    let archive = format!("conversation-{epoch}").into_bytes();
    let archive_sha256 = format!("{:x}", Sha256::digest(&archive));
    let capture = SandboxCheckpointConversation::new(
        AgentId("agt_a".into()),
        "conversation-a",
        ConversationSnapshot {
            conversation_id: "conversation-a".into(),
            archive_sha256,
            archive,
        },
    )
    .unwrap();
    SandboxCheckpointRef::seal(
        &CheckpointReceipt {
            epoch,
            object_reference: format!("checkpoints/sandbox-{epoch}.tar.zst"),
            sha256: format!("{epoch:064x}"),
            writers: WriterState::Resumed,
        },
        &[capture],
        sealed_at,
    )
    .unwrap()
}
