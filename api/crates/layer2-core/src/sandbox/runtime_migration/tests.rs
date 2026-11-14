use super::*;
use crate::sandbox::custom_runtime::test_support::{custom, image, pause, request};
use crate::sandbox::store::*;
use crate::sandbox::{Sandbox, SandboxStatus};
use crate::{Authorization, Caller, Capability};
use std::sync::{Arc, Mutex};

struct Authority {
    policy: RuntimeMigrationPolicy,
    entry: Mutex<Option<RuntimeMigrationAuthorityEntry>>,
}
#[async_trait::async_trait]
impl RuntimeMigrationAuthority for Authority {
    fn policy(&self) -> &RuntimeMigrationPolicy {
        &self.policy
    }
    async fn resolve(
        &self,
        _: &crate::SandboxId,
    ) -> Result<RuntimeMigrationAuthorityEntry, SandboxStoreError> {
        self.entry
            .lock()
            .unwrap()
            .clone()
            .ok_or(SandboxStoreError::Conflict)
    }
}
fn entry(sandbox: &Sandbox) -> RuntimeMigrationAuthorityEntry {
    let now = runtime_migration_now_ms().unwrap();
    RuntimeMigrationAuthorityEntry {
        authority_id: "approved-fixture".into(),
        request_key: "first-migration".into(),
        identity: RuntimeMigrationIdentity::capture(sandbox).unwrap(),
        target_image: image('b'),
        custom_migration: Some(request(sandbox, 'b', "first-migration")),
        clearance: RuntimeMigrationClearance::Paused,
        observation_reference: None,
        issued_at_ms: now,
        expires_at_ms: now + 30_000,
    }
}
fn policy(entry: &RuntimeMigrationAuthorityEntry) -> RuntimeMigrationPolicy {
    RuntimeMigrationPolicy {
        schema_version: 1,
        policy_id: "qualified-fixture".into(),
        deployment_id: "fixture".into(),
        scoped_sandboxes: vec![entry.identity.sandbox_id.clone()],
        targets: vec![RuntimeMigrationTarget {
            repository: "registry.example/runtime".into(),
            image: image('b'),
        }],
        qualified_sources: Vec::new(),
    }
}
fn admission(entry: &RuntimeMigrationAuthorityEntry) -> SandboxCustomRuntimeMigrationAdmission {
    SandboxCustomRuntimeMigrationAdmission {
        migration: entry.custom_migration.clone().unwrap(),
        expected_current_revision: entry.identity.revision,
        restart_failed: false,
        basis: RuntimeMigrationAdmissionBasis {
            authority_id: entry.authority_id.clone(),
            policy_id: "qualified-fixture".into(),
            request_key: entry.request_key.clone(),
            identity: entry.identity.clone(),
            authority_issued_at_ms: entry.issued_at_ms,
            authority_expires_at_ms: entry.expires_at_ms,
            approved_target: entry.target_image.clone(),
            source_runtime: entry.identity.runtime.clone(),
            clearance: entry.clearance.clone(),
            admitted_at: crate::now_rfc3339(),
            previous_request_keys: Vec::new(),
            earlier_admissions: Vec::new(),
        },
    }
}

#[test]
fn authority_bundle_refuses_duplicate_ids_keys_sources_and_target_substitution() {
    let value = entry(&custom());
    let policy = policy(&value);
    let bundle = RuntimeMigrationBundle {
        schema_version: 1,
        policy_id: policy.policy_id.clone(),
        entries: vec![value.clone()],
    };
    assert!(RuntimeMigrationBundle::parse(&serde_json::to_vec(&bundle).unwrap(), &policy).is_ok());
    let mut duplicate = bundle.clone();
    duplicate.entries.push(value.clone());
    assert!(
        RuntimeMigrationBundle::parse(&serde_json::to_vec(&duplicate).unwrap(), &policy).is_err()
    );
    let mut unknown = serde_json::to_value(&bundle).unwrap();
    unknown["client_clear"] = true.into();
    assert!(
        RuntimeMigrationBundle::parse(&serde_json::to_vec(&unknown).unwrap(), &policy).is_err()
    );
    let mut wrong_target = value.clone();
    wrong_target.target_image = image('c');
    assert!(policy
        .validate_entry(&wrong_target, runtime_migration_now_ms().unwrap())
        .is_err());
    let mut expired = value.clone();
    expired.expires_at_ms = expired.issued_at_ms - 1;
    assert!(policy
        .validate_entry(&expired, runtime_migration_now_ms().unwrap())
        .is_err());
    let mut wrong_family = policy.clone();
    wrong_family.targets[0].repository = "registry.example/runtime-copy".into();
    assert!(wrong_family.validate().is_err());
    let mut duplicate_mapping = policy.clone();
    duplicate_mapping
        .targets
        .push(duplicate_mapping.targets[0].clone());
    assert!(duplicate_mapping.validate().is_err());
}

#[tokio::test]
async fn supported_admission_is_atomic_fenced_and_replays_after_bundle_removal() {
    let mut original = custom();
    original.revision = 1;
    let store = Arc::new(InMemorySandboxStore::default());
    store.create_sandbox(original.clone()).await.unwrap();
    store
        .open_sandbox_session(&original.id, "session-fixture", "2099-01-01T00:00:00Z")
        .await
        .unwrap();
    let prior_revision = original.revision;
    pause(&mut original);
    original.revision = prior_revision + 1;
    store
        .save_sandbox(prior_revision, original.clone())
        .await
        .unwrap();
    let value = entry(&original);
    let authority = Arc::new(Authority {
        policy: policy(&value),
        entry: Mutex::new(Some(value.clone())),
    });
    let service = SandboxCustomRuntimeMigrationService::new(store.clone(), store.clone())
        .with_authority(authority.clone(), true);
    let command = RuntimeMigrationCommand {
        migration: value.custom_migration.clone().unwrap(),
        authority_id: value.authority_id.clone(),
        expected_current_revision: original.revision,
        restart_failed: false,
    };
    let owner = Caller {
        subject: original.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    assert!(matches!(
        service.request(&owner, &original.id, command.clone()).await,
        Err(RuntimeMigrationError::Forbidden)
    ));
    let operator = Caller {
        subject: original.owner.clone(),
        authorization: Authorization::new([Capability::ControlAnySandbox]),
    };
    let mut expired = admission(&value);
    expired.basis.authority_expires_at_ms = expired.basis.authority_issued_at_ms - 1;
    assert!(store.admit_custom_runtime_migration(expired).await.is_err());
    let mut stale = admission(&value);
    stale.expected_current_revision += 1;
    assert!(store.admit_custom_runtime_migration(stale).await.is_err());
    // An admitted durable desktop lease refuses even when this process has no
    // registered connection. The explicit lease is closed before admission.
    assert!(store
        .admit_custom_runtime_migration(admission(&value))
        .await
        .is_err());
    store
        .close_sandbox_session(&original.id, "session-fixture")
        .await
        .unwrap();
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(original.clone())
    );
    let admitted = service
        .request(&operator, &original.id, command.clone())
        .await
        .unwrap();
    assert_eq!(admitted.status, SandboxStatus::Rotating);
    assert_eq!(admitted.revision, original.revision + 1);
    assert_eq!(admitted.profile, original.profile);
    assert_eq!(admitted.lifecycle.home, original.lifecycle.home);
    assert_eq!(
        admitted.effective_custom_runtime_image(),
        Some(image('a').as_str())
    );
    assert!(admitted
        .runtime_update_request()
        .unwrap()
        .started_at
        .is_some());
    assert!(admitted
        .lifecycle
        .custom_runtime
        .as_ref()
        .unwrap()
        .admission_basis
        .is_some());
    *authority.entry.lock().unwrap() = None;
    assert_eq!(
        service
            .request(&operator, &original.id, command.clone())
            .await
            .unwrap(),
        admitted
    );
    let closed = SandboxCustomRuntimeMigrationService::new(store.clone(), store.clone());
    assert_eq!(
        closed
            .request(&operator, &original.id, command.clone())
            .await
            .unwrap(),
        admitted
    );
    let mut altered = command.clone();
    altered.migration.target_image = image('c');
    assert!(closed
        .request(&operator, &original.id, altered)
        .await
        .is_err());
    let mut changed_revision = command.clone();
    changed_revision.expected_current_revision += 1;
    assert!(closed
        .request(&operator, &original.id, changed_revision)
        .await
        .is_err());
    let mut changed_basis = admission(&value);
    changed_basis.basis.approved_target = image('c');
    assert!(store
        .admit_custom_runtime_migration(changed_basis)
        .await
        .is_err());
    let mut wrong_authority = command.clone();
    wrong_authority.authority_id = "other".into();
    assert!(closed
        .request(&operator, &original.id, wrong_authority)
        .await
        .is_err());
    let mut restart = command;
    restart.restart_failed = true;
    assert!(closed
        .request(&operator, &original.id, restart)
        .await
        .is_err());
    let mut stripped = admitted.clone();
    stripped.revision += 1;
    stripped.lifecycle.custom_runtime = None;
    assert!(store
        .save_sandbox(admitted.revision, stripped)
        .await
        .is_err());
    let fresh_store = InMemorySandboxStore::default();
    assert!(fresh_store.create_sandbox(admitted.clone()).await.is_err());
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(admitted)
    );
}
