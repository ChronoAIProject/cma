use super::*;
use layer2_core::{profiles::*, sandbox::*, SandboxProfileId, SubjectId};
const NOW: &str = "2026-09-05T00:30:01Z";
const IDLE_AT: &str = "2026-09-05T00:15:00Z";
pub(super) fn sandbox() -> Sandbox {
    let mut value = Sandbox {
        id: SandboxId("sbx_0123456789abcdef0123456789abcdef".into()),
        owner: SubjectId("6e660487-dba4-4be9-ae06-4593b41c4a30".into()),
        name: "lifecycle".into(),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_lifecycle".into()),
            revision: 3,
            digest: "a".repeat(64),
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
    };
    value.lifecycle.runtime_generation_high_water = 1;
    let mut home = SandboxHome::reserve(
        &value.id,
        layer3_adapter::HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "test".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "one".into(),
        },
    )
    .unwrap();
    home.receipt = Some(layer3_adapter::HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "home".into(),
        storage_generation: "uid".into(),
    });
    value.lifecycle.home = Some(Box::new(home));
    value
}
