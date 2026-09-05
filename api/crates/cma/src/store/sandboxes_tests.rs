use layer2_core::ids::{SandboxId, SandboxProfileId, ServiceId, SubjectId};
use layer2_core::profiles::{
    AgentProvider, ModelAccess, SandboxProfileContent, SandboxProfileSnapshot,
};
use layer2_core::sandbox::{
    Sandbox, SandboxCredentialState, SandboxLimits, SandboxOrigin, SandboxSignInState,
    SandboxStatus, SandboxWorkState, SandboxWorkStep,
};
use mongodb::bson::Bson;

use super::sandboxes::{
    admitted_sandbox_document, read_sandbox, sandbox_document, sandbox_replacement_fields,
};

const NOW: &str = "2026-09-05T00:00:00Z";

fn sandbox(origin: SandboxOrigin) -> Sandbox {
    Sandbox {
        id: SandboxId("sbx_schema".into()),
        owner: SubjectId("owner".into()),
        name: "research".into(),
        origin,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_schema".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: SandboxProfileContent {
                cpu_millis: 1_000,
                memory_mib: 2_048,
                nyxid_services: vec![ServiceId::new("profile-service")],
                agent_provider: AgentProvider::Codex,
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 4,
                max_agents_per_workspace: 4,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Provisioning,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            reaches: vec![ServiceId::new("profile-service")],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, NOW.into()),
        limits: SandboxLimits {
            max_workspaces: 4,
            max_agents_per_workspace: 4,
        },
        checkpoint: None,
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

#[test]
fn manual_origin_query_fields_are_explicit_and_fingerprint_free() {
    let sandbox = sandbox(SandboxOrigin::Manual);
    let document = sandbox_document(&sandbox, 0).unwrap();

    assert_eq!(document.get_str("origin_type").unwrap(), "manual");
    assert_eq!(document.get("origin_launch_id"), Some(&Bson::Null));
    assert_eq!(document.get("admission_fingerprint"), Some(&Bson::Null));
    assert_eq!(read_sandbox(&document).unwrap(), sandbox);
}

#[test]
fn trigger_origin_and_fingerprint_round_trip_and_survive_replacement() {
    let sandbox = sandbox(SandboxOrigin::Trigger {
        launch_id: "launch-schema".into(),
    });
    let fingerprint = "b".repeat(64);
    let document = admitted_sandbox_document(&sandbox, 1, &fingerprint).unwrap();

    assert_eq!(document.get_str("origin_type").unwrap(), "trigger");
    assert_eq!(
        document.get_str("origin_launch_id").unwrap(),
        "launch-schema"
    );
    assert_eq!(
        document.get_str("admission_fingerprint").unwrap(),
        fingerprint
    );
    assert_eq!(read_sandbox(&document).unwrap(), sandbox);
    let replacement = sandbox_replacement_fields(&sandbox).unwrap();
    assert!(!replacement.contains_key("admission_fingerprint"));
    assert_eq!(replacement.get_str("origin_type").unwrap(), "trigger");
    assert!(sandbox_document(&sandbox, 0).is_err());
}

#[test]
fn trigger_query_fields_and_fingerprint_are_independently_validated() {
    let sandbox = sandbox(SandboxOrigin::Trigger {
        launch_id: "launch-schema".into(),
    });
    let document = admitted_sandbox_document(&sandbox, 1, &"c".repeat(64)).unwrap();
    for (field, value) in [
        ("origin_type", Bson::String("manual".into())),
        ("origin_launch_id", Bson::String("another-launch".into())),
        ("admission_fingerprint", Bson::Null),
        ("admission_fingerprint", Bson::String("C".repeat(64))),
    ] {
        let mut corrupt = document.clone();
        corrupt.insert(field, value);
        assert!(read_sandbox(&corrupt).is_err(), "accepted corrupt {field}");
    }
}
