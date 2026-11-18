use super::*;
use crate::sandbox::{
    lifecycle_test_support, SandboxLifecycleOperation, SandboxReviveCause, SandboxRevivePhase,
};

fn at(offset: u64) -> SystemTime {
    UNIX_EPOCH
        + Duration::from_secs(
            crate::seconds_from_rfc3339(lifecycle_test_support::NOW).unwrap() + offset,
        )
}

#[test]
fn durable_admission_bounds_observation_without_cancelling_slow_reopen() {
    let mut sandbox = lifecycle_test_support::sandbox();
    sandbox.lifecycle.checkpoints.latest = Some(lifecycle_test_support::checkpoint(
        1,
        lifecycle_test_support::NOW,
    ));
    sandbox
        .lifecycle
        .start_revive(
            &sandbox.id,
            lifecycle_test_support::NOW.into(),
            SandboxReviveCause::ExplicitRequest,
        )
        .unwrap();
    sandbox.status = SandboxStatus::Reviving;
    let before = SandboxLifecycleResult::observe(&sandbox, None, at(89), true).unwrap();
    assert_eq!(before.state, SandboxLifecycleResultState::InProgress);
    let overdue = SandboxLifecycleResult::observe(&sandbox, None, at(90), true).unwrap();
    assert_eq!(
        overdue.state,
        SandboxLifecycleResultState::RecoverablePending
    );
    assert_eq!(overdue.code, Some("lifecycle_wait_exceeded"));
    assert!(overdue.continuing);
    assert_eq!(overdue.retry_after_seconds, Some(5));
    assert!(overdue.actions.contains(&SandboxLifecycleAction::Retire));
    if let Some(SandboxLifecycleOperation::Revive { phase, .. }) = &mut sandbox.lifecycle.operation
    {
        *phase = SandboxRevivePhase::ConversationReopen;
    }
    sandbox.updated_at = crate::rfc3339_at(at(200));
    sandbox.work.ready_at = crate::rfc3339_at(at(230));
    sandbox.work.attempts = 10;
    let replica: Sandbox = serde_json::from_str(&serde_json::to_string(&sandbox).unwrap()).unwrap();
    for elapsed in [90, 119, 120, 299, 301, 960] {
        assert_eq!(
            SandboxLifecycleResult::observe(&replica, None, at(elapsed), true).unwrap(),
            overdue
        );
    }
    assert_eq!(sandbox.status, SandboxStatus::Reviving);
    sandbox.lifecycle.cancel_operation();
    sandbox.status = SandboxStatus::Active;
    assert!(SandboxLifecycleResult::observe(&sandbox, None, at(200), true).is_none());
}

fn failed_sandbox() -> (Sandbox, SandboxLifecycleResult) {
    let mut sandbox = lifecycle_test_support::sandbox();
    sandbox
        .begin_pause(lifecycle_test_support::NOW.into(), None)
        .unwrap();
    let admission = SandboxLifecycleResult::observe(&sandbox, None, at(0), true).unwrap();
    sandbox
        .work
        .claim(
            "worker".into(),
            &crate::rfc3339_at(at(1)),
            crate::rfc3339_at(at(60)),
        )
        .unwrap();
    let fence = sandbox.work.lease.as_ref().unwrap().fence;
    sandbox
        .fail_lifecycle(
            "runtime rejected lifecycle operation".into(),
            "worker",
            fence,
            crate::rfc3339_at(at(2)),
        )
        .unwrap();
    sandbox.validate().unwrap();
    (sandbox, admission)
}

#[test]
fn failure_and_retirement_never_advertise_unavailable_retries_or_retirement() {
    let (sandbox, admission) = failed_sandbox();
    let sandbox_bytes = serde_json::to_vec(&sandbox).unwrap();
    let mut sandbox: Sandbox = serde_json::from_slice(&sandbox_bytes).unwrap();
    let failed = SandboxLifecycleResult::observe(&sandbox, None, at(2), true).unwrap();
    assert_eq!(failed.operation_id, admission.operation_id);
    assert_eq!(failed.started_at, admission.started_at);
    assert_eq!(failed.result_due_at, admission.result_due_at);
    assert_eq!(failed.state, SandboxLifecycleResultState::Failed);
    assert!(!failed.continuing);
    assert_eq!(failed.retry_after_seconds, None);
    assert_eq!(
        SandboxLifecycleResult::observe(&sandbox, None, at(600), true).unwrap(),
        failed
    );
    sandbox.updated_at = sandbox.created_at.clone();
    sandbox.validate().unwrap();
    assert_eq!(
        SandboxLifecycleResult::observe(&sandbox, None, at(600), true).unwrap(),
        failed
    );
    assert!(serde_json::to_value(&failed)
        .unwrap()
        .get("retry_after_seconds")
        .is_none());
    assert_eq!(
        SandboxLifecycleResult::observe(&sandbox, None, at(0), false)
            .unwrap()
            .actions,
        vec![SandboxLifecycleAction::Observe]
    );
    sandbox.begin_retirement(crate::rfc3339_at(at(3))).unwrap();
    assert!(sandbox.lifecycle.failed_operation.is_none());
    assert!(SandboxLifecycleResult::observe(&sandbox, None, at(100), true).is_none());
    let retirement =
        SandboxLifecycleResult::observe(&sandbox, Some(lifecycle_test_support::NOW), at(90), true)
            .unwrap();
    assert_eq!(
        retirement.state,
        SandboxLifecycleResultState::RecoverablePending
    );
    assert_eq!(retirement.actions, vec![SandboxLifecycleAction::Observe]);
    sandbox.updated_at = crate::rfc3339_at(at(500));
    assert_eq!(
        SandboxLifecycleResult::observe(&sandbox, Some(lifecycle_test_support::NOW), at(500), true)
            .unwrap(),
        retirement
    );
}

#[test]
fn failed_admission_rejects_mismatched_identity_or_time_and_reads_legacy_records() {
    let (sandbox, _) = failed_sandbox();
    let encoded = serde_json::to_value(&sandbox).unwrap();
    for (field, value) in [
        ("operation_id", "sandbox-lifecycle-v1:another:1"),
        ("operation_id", "sandbox-lifecycle-v1:sbx_lifecycle:2"),
        ("requested_at", "invalid"),
        ("requested_at", ""),
    ] {
        let mut invalid = encoded.clone();
        invalid["lifecycle"]["failed_operation"][field] = value.into();
        let invalid: Sandbox = serde_json::from_value(invalid).unwrap();
        assert!(invalid.validate().is_err(), "accepted {field}={value}");
    }
    let mut invalid = sandbox.clone();
    invalid.status = SandboxStatus::Active;
    invalid.failure = None;
    assert!(invalid.validate().is_err());
    let mut legacy = encoded;
    legacy["lifecycle"]
        .as_object_mut()
        .unwrap()
        .remove("failed_operation");
    let legacy: Sandbox = serde_json::from_value(legacy).unwrap();
    legacy.validate().unwrap();
    assert!(legacy.lifecycle.failed_operation.is_none());
}

#[test]
fn provisioning_and_installing_share_the_creation_deadline() {
    let mut sandbox = lifecycle_test_support::sandbox();
    sandbox.created_at = lifecycle_test_support::NOW.into();
    sandbox.status = SandboxStatus::Provisioning;
    let first = SandboxLifecycleResult::observe(&sandbox, None, at(90), true).unwrap();
    sandbox.status = SandboxStatus::Installing;
    sandbox.updated_at = crate::rfc3339_at(at(200));
    assert_eq!(
        SandboxLifecycleResult::observe(&sandbox, None, at(200), true).unwrap(),
        first
    );
}
