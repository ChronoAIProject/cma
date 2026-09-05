use layer2_core::ids::{SandboxId, SandboxProfileId, ServiceId, SubjectId};
use layer2_core::profiles::{
    content_digest, AgentProvider, ModelAccess, SandboxProfileContent, SandboxProfileSnapshot,
};
use layer2_core::sandbox::store::{SandboxAdmission, SandboxAdmissionName};
use layer2_core::sandbox::{
    Sandbox, SandboxCredentialState, SandboxLimits, SandboxOrigin, SandboxSignInState,
    SandboxStatus, SandboxWorkState, SandboxWorkStep,
};
use mongodb::bson::{doc, Bson};

use super::sandboxes::{
    admitted_sandbox_document, read_sandbox, read_sandbox_admission_replay, sandbox_document,
    sandbox_indexes, sandbox_replacement_fields,
};

const NOW: &str = "2026-09-05T00:00:00Z";

fn sandbox(origin: SandboxOrigin) -> Sandbox {
    let content = SandboxProfileContent {
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
    };
    Sandbox {
        id: SandboxId("sbx_schema".into()),
        owner: SubjectId("owner".into()),
        name: "research".into(),
        origin,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_schema".into()),
            revision: 1,
            digest: content_digest(&content).unwrap(),
            content,
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
        lifecycle: Default::default(),
        last_activity_at: NOW.into(),
        created_at: NOW.into(),
        updated_at: NOW.into(),
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

fn trigger_document(sandbox: &Sandbox) -> (mongodb::bson::Document, SandboxAdmission) {
    let admission = SandboxAdmission::new(
        sandbox.clone(),
        SandboxAdmissionName::Allocate("research".into()),
        1,
        NOW.into(),
    )
    .unwrap();
    let document = admitted_sandbox_document(
        sandbox,
        1,
        &admission.name,
        &admission.admission_fingerprint,
    )
    .unwrap();
    (document, admission)
}

#[test]
fn manual_origin_query_fields_are_explicit_and_fingerprint_free() {
    let sandbox = sandbox(SandboxOrigin::Manual);
    let document = sandbox_document(&sandbox, 0).unwrap();

    assert_eq!(document.get_str("origin_type").unwrap(), "manual");
    assert_eq!(document.get("origin_launch_id"), Some(&Bson::Null));
    assert_eq!(document.get("admission_name"), Some(&Bson::Null));
    assert_eq!(document.get("admission_fingerprint"), Some(&Bson::Null));
    assert_eq!(read_sandbox(&document).unwrap(), sandbox);
    assert!(read_sandbox_admission_replay(&document).is_err());
}

#[test]
fn trigger_origin_and_fingerprint_round_trip_and_survive_replacement() {
    let sandbox = sandbox(SandboxOrigin::Trigger {
        launch_id: "launch-schema".into(),
    });
    let (document, admission) = trigger_document(&sandbox);

    assert_eq!(document.get_str("origin_type").unwrap(), "trigger");
    assert_eq!(
        document.get_str("origin_launch_id").unwrap(),
        "launch-schema"
    );
    assert_eq!(
        document.get_str("admission_fingerprint").unwrap(),
        admission.admission_fingerprint
    );
    assert_eq!(
        document.get_document("admission_name").unwrap(),
        &doc! { "mode": "allocate", "value": "research" }
    );
    assert_eq!(read_sandbox(&document).unwrap(), sandbox);
    let replay = read_sandbox_admission_replay(&document).unwrap();
    assert_eq!(replay.sandbox, sandbox);
    assert_eq!(replay.name, admission.name);
    assert_eq!(
        replay.admission_fingerprint,
        admission.admission_fingerprint
    );
    let replacement = sandbox_replacement_fields(&sandbox).unwrap();
    assert!(!replacement.contains_key("admission_name"));
    assert!(!replacement.contains_key("admission_fingerprint"));
    assert_eq!(replacement.get_str("origin_type").unwrap(), "trigger");
    assert!(sandbox_document(&sandbox, 0).is_err());
}

#[test]
fn trigger_query_fields_and_fingerprint_are_independently_validated() {
    let sandbox = sandbox(SandboxOrigin::Trigger {
        launch_id: "launch-schema".into(),
    });
    let (document, _) = trigger_document(&sandbox);
    for (field, value) in [
        ("origin_type", Bson::String("manual".into())),
        ("origin_launch_id", Bson::String("another-launch".into())),
        ("admission_name", Bson::Null),
        (
            "admission_name",
            Bson::Document(doc! { "mode": "allocate", "value": "changed" }),
        ),
        (
            "admission_name",
            Bson::Document(doc! { "mode": "allocate", "value": "research", "unexpected": true }),
        ),
        ("admission_fingerprint", Bson::Null),
        ("admission_fingerprint", Bson::String("C".repeat(64))),
    ] {
        let mut corrupt = document.clone();
        corrupt.insert(field, value);
        assert!(read_sandbox(&corrupt).is_err(), "accepted corrupt {field}");
    }
}

#[test]
fn sandbox_indexes_match_the_schema_contract() {
    let indexes = sandbox_indexes();
    assert_eq!(indexes[0].keys, doc! { "owner": 1, "name": 1 });
    let options = indexes[0].options.as_ref().unwrap();
    assert_eq!(options.unique, Some(true));
    assert_eq!(
        options.partial_filter_expression,
        Some(doc! { "retired_at": Bson::Null })
    );
    assert_eq!(indexes[1].keys, doc! { "owner": 1, "origin_launch_id": 1 });
    let options = indexes[1].options.as_ref().unwrap();
    assert_eq!(options.unique, Some(true));
    assert_eq!(
        options.name.as_deref(),
        Some("sandboxes_owner_trigger_launch")
    );
    assert_eq!(
        options.partial_filter_expression,
        Some(doc! { "origin_type": "trigger" })
    );
    assert_eq!(
        indexes[2].keys,
        doc! { "work_pending": 1, "work_ready_at": 1, "work_lease_expires_at": 1 }
    );
    assert_eq!(
        indexes[3].keys,
        doc! { "owner": 1, "status": 1, "updated_at": -1 }
    );
    assert_eq!(
        indexes[4].keys,
        doc! {
            "owner": 1,
            "status": 1,
            "runtime_version": 1,
            "_id": 1,
        }
    );
    assert_eq!(
        indexes[4].options.as_ref().unwrap().name.as_deref(),
        Some("sandboxes_operator_inventory")
    );
    assert_eq!(indexes[5].keys, doc! { "owner": 1, "_id": 1 });
    assert_eq!(
        indexes[5].options.as_ref().unwrap().name.as_deref(),
        Some("sandboxes_owner_page")
    );
    assert_eq!(indexes[6].keys, doc! { "owner": 1, "status": 1, "_id": 1 });
    assert_eq!(
        indexes[6].options.as_ref().unwrap().name.as_deref(),
        Some("sandboxes_owner_status_page")
    );
}
