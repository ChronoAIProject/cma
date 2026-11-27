use super::*;
use crate::sandbox::lifecycle_test_support::{sandbox, NOW};
use crate::sandbox::SandboxHome;
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt};

fn fixture() -> (Sandbox, RuntimeStatus) {
    let mut sandbox = sandbox();
    let mut home = SandboxHome::reserve(
        &sandbox.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "storage".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "seed-original".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "original-home".into(),
        storage_generation: "original-volume".into(),
    })
    .unwrap();
    let status = RuntimeStatus {
        agent_filesystem: false,
        agent_attachment_upload: false,
        agent_skill_install: false,
        agent_skill_upgrade: false,
        agent_model_catalog: false,
        agent_browser: false,
        agent_resource_preparation: false,
        agent_process_restart: false,
        exact_github_credential_route: false,
        github_repository_scope: false,
        sandbox_id: sandbox.id.to_string(),
        generation: 1,
        home_identity: home.attachment().unwrap().identity().unwrap(),
        boot_id: "a".repeat(32),
        installation_ready: true,
        credential_ready: true,
        restore_pending: false,
        agent_processes: 1,
    };
    sandbox.lifecycle.home = Some(Box::new(home));
    sandbox.lifecycle.runtime_generation_high_water = 1;
    sandbox.runtime.as_mut().unwrap().provision_generation = Some(1);
    sandbox.validate().unwrap();
    (sandbox, status)
}

#[test]
fn process_identity_is_durable_and_cannot_be_rebound_on_a_provider_handle() {
    let (mut sandbox, status) = fixture();
    let binding = sandbox.runtime.clone().unwrap();
    let original = sandbox.clone();
    sandbox
        .record_runtime_boot(&binding, &status, NOW.into())
        .unwrap();
    assert_eq!(sandbox.revision, original.revision + 1);
    assert_eq!(
        sandbox.runtime.as_ref().unwrap().boot_id,
        Some(status.boot_id.clone())
    );
    let reopened: Sandbox = serde_json::from_slice(&serde_json::to_vec(&sandbox).unwrap()).unwrap();
    assert_eq!(reopened, sandbox);
    let binding = sandbox.runtime.clone().unwrap();
    sandbox
        .record_runtime_boot(&binding, &status, NOW.into())
        .unwrap();
    assert_eq!(sandbox, reopened);
    let mut replacement = status.clone();
    replacement.boot_id = "b".repeat(32);
    assert!(!sandbox.runtime_process_matches(&replacement).unwrap());
    assert!(sandbox
        .record_runtime_boot(&binding, &replacement, NOW.into())
        .is_err());
    assert_eq!(sandbox, reopened);
    for next_boot in [None, Some(replacement.boot_id)] {
        let mut forged = sandbox.clone();
        forged.runtime.as_mut().unwrap().boot_id = next_boot;
        assert!(sandbox
            .validate_runtime_generation_successor(&forged)
            .is_err());
    }
}

#[test]
fn foreign_home_generation_and_malformed_status_cannot_authorize_a_process() {
    let (mut sandbox, status) = fixture();
    let original = sandbox.clone();
    let mut variations = Vec::new();
    let mut foreign = status.clone();
    foreign.sandbox_id = "another-sandbox".into();
    variations.push(foreign);
    let mut foreign = status.clone();
    foreign.home_identity = "c".repeat(43);
    variations.push(foreign);
    let mut foreign = status.clone();
    foreign.generation = 2;
    variations.push(foreign);
    let mut foreign = status.clone();
    foreign.boot_id = "A".repeat(32);
    variations.push(foreign);
    let mut foreign = status.clone();
    foreign.credential_ready = false;
    variations.push(foreign);
    for foreign in variations {
        assert!(sandbox.runtime_process_matches(&foreign).is_err());
        assert!(sandbox
            .record_runtime_boot(original.runtime.as_ref().unwrap(), &foreign, NOW.into())
            .is_err());
        assert_eq!(sandbox, original);
    }
}

#[test]
fn legacy_bindings_roundtrip_without_process_identity_and_missing_storage_refuses_new_binding() {
    let legacy = sandbox();
    let encoded = serde_json::to_value(&legacy).unwrap();
    assert!(encoded["runtime"].get("boot_id").is_none());
    let decoded: Sandbox = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, legacy);
    let (mut sandbox, status) = fixture();
    sandbox.lifecycle.home = None;
    assert!(sandbox.runtime_process_matches(&status).is_err());
    sandbox.runtime.as_mut().unwrap().boot_id = Some(status.boot_id);
    assert!(sandbox.validate().is_err());
}
