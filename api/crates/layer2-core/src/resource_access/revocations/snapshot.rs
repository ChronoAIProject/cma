use crate::{profiles::*, sandbox::*, SandboxId, SandboxProfileId, SubjectId};
const NOW: &str = "2026-01-01T00:00:00Z";
fn sandbox_snapshot() -> SandboxProfileSnapshot {
    SandboxProfileSnapshot {
        id: SandboxProfileId("sbp_test".into()),
        revision: 1,
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
            software_install_commands: vec![],
            environment_variables: vec![],
        },
    }
}

pub(super) fn sandbox() -> Sandbox {
    Sandbox {
        id: SandboxId("sandbox".into()),
        owner: SubjectId("owner".into()),
        name: "research".into(),
        origin: SandboxOrigin::Manual,
        profile: sandbox_snapshot(),
        status: SandboxStatus::Active,
        failure: None,
        runtime: None,
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
                issued_at: NOW.into(),
                expires_at: "2026-10-05T00:00:00Z".into(),
                grant_fingerprint: "a".repeat(64),
            }),
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::WaitForCredentialRotation, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}
