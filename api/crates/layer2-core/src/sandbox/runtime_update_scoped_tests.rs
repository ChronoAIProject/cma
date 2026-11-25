use super::*;
use crate::sandbox::custom_runtime::test_support::{
    finish, image, pause, provision, step, END, HOLDER,
};
use crate::sandbox::store::{
    apply_scoped_image_rotation, runtime_migration_now_ms, Page, PageRequest,
    RuntimeMigrationAdmissionBasis, RuntimeMigrationClearance, SandboxImageRotationRequest,
    SandboxMaintenanceApplication, SandboxStore,
};
use crate::sandbox::{
    RuntimeMigrationIdentity, SandboxRuntimeUpdateError, SandboxRuntimeUpdateService,
};
use crate::{Authorization, Caller, Capability, SandboxId, SubjectId};
use std::sync::Arc;

fn scoped() -> Sandbox {
    let mut value = native();
    pause(&mut value);
    value
        .request_runtime_update(image('b'), false, NOW.into())
        .unwrap();
    let now = runtime_migration_now_ms().unwrap();
    let basis = RuntimeMigrationAdmissionBasis {
        authority_id: "scoped-rotation-fixture".into(),
        policy_id: "scoped-policy-fixture".into(),
        request_key: "scoped-request-fixture".into(),
        identity: RuntimeMigrationIdentity::capture(&value).unwrap(),
        authority_issued_at_ms: now,
        authority_expires_at_ms: now + 30_000,
        approved_target: image('b'),
        source_runtime: value.retained_runtime_binding().cloned(),
        clearance: RuntimeMigrationClearance::Paused,
        admitted_at: NOW.into(),
        previous_request_keys: vec![],
        earlier_admissions: vec![],
    };
    let SandboxMaintenanceApplication::Persist { sandbox, .. } = apply_scoped_image_rotation(
        &value,
        &[],
        &SandboxImageRotationRequest {
            sandbox_id: value.id.clone(),
            requested_at: NOW.into(),
            expected_runtime_version: image('b'),
        },
        &basis,
    )
    .unwrap() else {
        panic!("fixture must enter through scoped admission");
    };
    assert_eq!(
        sandbox.lifecycle.runtime_migration_admission.as_deref(),
        Some(&basis)
    );
    sandbox.validate().unwrap();
    *sandbox
}

fn reopen() -> Sandbox {
    let mut value = scoped();
    provision(&mut value, 'b');
    step(&mut value, |v, f| {
        v.complete_lifecycle_credential_delivery(HOLDER, f, NOW.into())
    });
    step(&mut value, |v, f| {
        v.complete_lifecycle_home_restore(HOLDER, f, NOW.into())
    });
    value
}

fn park(value: &mut Sandbox) {
    let fence = value.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    value
        .park_failed_runtime_update(failure("terminal_validation"), HOLDER, fence, NOW.into())
        .unwrap();
    value.validate().unwrap();
}

fn held(value: Sandbox) -> SandboxUnpublishedRecoverySnapshot {
    let held = SandboxUnpublishedRecoverySnapshot {
        sandbox: value,
        agents: vec![],
        workspaces: vec![],
    };
    held.validate().unwrap();
    held
}

#[test]
fn scoped_failed_retry_preserves_original_request_and_phase() {
    for mut value in [scoped(), reopen()] {
        park(&mut value);
        let before = value.clone();
        for target in [image('b'), image('c')] {
            assert!(value
                .request_runtime_update(target, false, NOW.into())
                .is_err());
            assert_eq!(value, before);
        }
    }
}

#[tokio::test]
async fn scoped_explicit_service_retry_refuses_before_recovery_inspection() {
    let mut early = scoped();
    park(&mut early);
    let mut failed = reopen();
    park(&mut failed);
    assert!(failed.unpublished_recovery_candidate());
    let mut stalled = reopen();
    let fence = stalled.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    stalled
        .retry_lifecycle(NOW.into(), HOLDER, fence, NOW.into())
        .unwrap();
    assert!(stalled.unpublished_recovery_candidate());
    assert!(stalled.runtime_update_request().unwrap().failure.is_none());
    for value in [early, failed, stalled] {
        let service = service(&value);
        assert!(!service.can_update(&value));
        for capability in [Capability::UseOwnSandboxes, Capability::ControlAnySandbox] {
            let caller = Caller::new(value.owner.clone(), Authorization::new([capability]));
            let result = if capability == Capability::UseOwnSandboxes {
                service.request(&caller, &value.id, &image('b')).await
            } else {
                service
                    .request_as_operator(&caller, &value.id, &image('b'))
                    .await
            };
            assert!(matches!(
                result,
                Err(SandboxRuntimeUpdateError::NotAvailable)
            ));
        }
    }
}

#[test]
fn scoped_explicit_recovery_transition_refuses() {
    let mut value = reopen();
    park(&mut value);
    let current = held(value);
    let mut requested = request(current.clone());
    requested.target_runtime_version = image('b');
    requested.proof.validate_for(&current).unwrap();
    assert!(apply_unpublished_runtime_recovery(&current, &requested).is_err());
    assert_eq!(current, requested.snapshot);
}

#[test]
fn scoped_automatic_recovery_keeps_existing_target_once_only() {
    let mut value = reopen();
    park(&mut value);
    let current = held(value);
    let mut requested = request(current.clone());
    requested.automatic = true;
    requested.target_runtime_version = image('c');
    assert!(apply_unpublished_runtime_recovery(&current, &requested).is_err());
    requested.target_runtime_version = image('b');
    let after = apply_unpublished_runtime_recovery(&current, &requested)
        .unwrap()
        .sandbox;
    assert_eq!(
        after.lifecycle.runtime_migration_admission,
        current.sandbox.lifecycle.runtime_migration_admission
    );
    assert_eq!(
        after.admitted_runtime_update_target(),
        Some(image('b').as_str())
    );
    assert!(after.lifecycle.unpublished_recovery_attempted);
    assert!(after.runtime_update_request().unwrap().failure.is_none());
    let mut repeated = current.clone();
    repeated.sandbox.lifecycle.unpublished_recovery_attempted = true;
    requested.snapshot = repeated.clone();
    assert!(apply_unpublished_runtime_recovery(&repeated, &requested).is_err());
}

#[tokio::test]
async fn scoped_accepted_request_replay_does_not_schedule_work() {
    let mut value = scoped();
    let before = value.clone();
    assert!(!value
        .request_runtime_update(image('b'), false, NOW.into())
        .unwrap());
    assert_eq!(value, before);
    let caller = Caller::new(
        value.owner.clone(),
        Authorization::new([Capability::ControlAnySandbox]),
    );
    assert_eq!(
        service(&value)
            .request_as_operator(&caller, &value.id, &image('b'))
            .await
            .unwrap(),
        before
    );
}

#[test]
fn completed_scoped_operation_allows_new_normal_request() {
    let mut value = scoped();
    assert!(value.cancel_recovery_pause(NOW).unwrap());
    provision(&mut value, 'b');
    finish(&mut value);
    assert_eq!(value.status, SandboxStatus::Active);
    assert!(value
        .runtime_update_request()
        .unwrap()
        .completed_at
        .is_some());
    let basis = value.lifecycle.runtime_migration_admission.clone();
    assert!(basis.is_some());
    assert!(service(&value).can_update(&value));
    assert!(value
        .request_runtime_update(image('c'), false, NOW.into())
        .unwrap());
    assert_eq!(value.lifecycle.runtime_migration_admission, basis);
    assert_eq!(
        value.runtime_update_request().unwrap().target_version,
        image('c')
    );
}

#[test]
fn ordinary_unscoped_failed_retry_and_recovery_remain_available() {
    let current = snapshot(true);
    assert!(current
        .sandbox
        .lifecycle
        .runtime_migration_admission
        .is_none());
    let mut retried = current.sandbox.clone();
    assert!(retried
        .request_runtime_update("image-v2".into(), false, NOW.into())
        .unwrap());
    assert_eq!(
        retried.lifecycle.operation,
        current.sandbox.lifecycle.operation
    );
    assert!(retried.runtime_update_request().unwrap().failure.is_none());
    assert_eq!(retried.work.step, SandboxWorkStep::RunLifecycleOperation);
    assert!(apply_unpublished_runtime_recovery(&current, &request(current.clone())).is_ok());
}

// This read-only service fixture contains a Sandbox created by the supported
// scoped transition above. Refusal and replay may only read that exact value.
struct ReadOnlyStore(Sandbox);

#[async_trait::async_trait]
impl SandboxStore for ReadOnlyStore {
    async fn create_sandbox(&self, _: Sandbox) -> Result<(), SandboxStoreError> {
        panic!("no generic authority creation")
    }
    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        assert_eq!(id, &self.0.id);
        Ok(Some(self.0.clone()))
    }
    async fn save_sandbox(&self, _: u64, _: Sandbox) -> Result<(), SandboxStoreError> {
        panic!("refusal and replay must not persist work")
    }
    async fn list_sandboxes(
        &self,
        _: &SubjectId,
        _: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        panic!("no inventory read")
    }
    async fn unpublished_runtime_recovery_snapshot(
        &self,
        _: &SandboxId,
    ) -> Result<SandboxUnpublishedRecoverySnapshot, SandboxStoreError> {
        panic!("explicit scoped retry must refuse before recovery inspection")
    }
}

struct NoVerifier;
#[async_trait::async_trait]
impl SandboxUnpublishedRecoveryVerifier for NoVerifier {
    async fn verify(
        &self,
        _: &SandboxUnpublishedRecoverySnapshot,
    ) -> Result<SandboxUnpublishedRecoveryProof, SandboxStoreError> {
        panic!("explicit scoped retry must refuse before native effects")
    }
}

fn service(value: &Sandbox) -> SandboxRuntimeUpdateService {
    SandboxRuntimeUpdateService::new(Arc::new(ReadOnlyStore(value.clone())), image('c'))
        .with_unpublished_recovery(Arc::new(NoVerifier))
}
