use super::*;
mod parked_runtime_update;
mod predecessor;
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
fn exhausted_retirement_preserves_the_original_operation_and_reports_terminal_failure() {
    let mut sandbox = lifecycle_test_support::sandbox();
    let admitted = lifecycle_test_support::NOW;
    sandbox.begin_retirement(admitted.into()).unwrap();
    let pending = SandboxLifecycleResult::observe(&sandbox, Some(admitted), at(90), true).unwrap();
    assert!(pending.continuing);

    sandbox.work.attempts = crate::sandbox::work_retry::WORK_RETRY_EXTERNAL_BUDGET;
    let fence = sandbox
        .work
        .claim(
            "worker".into(),
            &crate::rfc3339_at(at(120)),
            crate::rfc3339_at(at(180)),
        )
        .unwrap();
    sandbox.work.stop_retirement("worker", fence).unwrap();
    sandbox.validate().unwrap();
    let failed = SandboxLifecycleResult::observe(&sandbox, Some(admitted), at(120), true).unwrap();
    assert_eq!(failed.state, SandboxLifecycleResultState::Failed);
    assert_eq!(failed.code, Some("retirement_retry_exhausted"));
    assert_eq!(failed.blocked_retirement_step, Some("stop_agents"));
    assert_eq!(failed.owner_slot_released, Some(false));
    assert!(!failed.continuing);
    assert_eq!(failed.retry_after_seconds, None);
    assert_eq!(failed.operation_id, pending.operation_id);
    assert_eq!(failed.started_at, pending.started_at);
    assert_eq!(failed.actions, [SandboxLifecycleAction::Observe]);
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
fn failed_lifecycle_writer_remains_readable_by_the_predecessor() {
    let (sandbox, _) = failed_sandbox();
    let predecessor = serde_json::from_value::<predecessor::PredecessorLifecycle>(
        serde_json::to_value(&sandbox.lifecycle).unwrap(),
    )
    .expect("a rolling predecessor must read a newly failed lifecycle");
    let mut old_writer = serde_json::to_value(&sandbox).unwrap();
    old_writer["lifecycle"] = serde_json::to_value(predecessor).unwrap();
    let new_reader: Sandbox = serde_json::from_value(old_writer).unwrap();
    new_reader.validate().unwrap();
    assert_eq!(new_reader, sandbox);
}

#[test]
fn retained_failure_without_admission_history_has_an_explicit_result() {
    let (sandbox, _) = failed_sandbox();
    let bytes = serde_json::to_value(sandbox).unwrap();
    assert!(bytes["lifecycle"].get("failed_operation").is_none());
    let retained: Sandbox = serde_json::from_value(bytes).unwrap();
    let result = SandboxLifecycleResult::observe(&retained, None, at(600), true)
        .expect("retained failures must not silently omit the result");
    assert_eq!(result.state, SandboxLifecycleResultState::Failed);
    assert!(!result.continuing);
    assert_eq!(result.code, Some("lifecycle_failed_admission_unavailable"));
    assert!(result.operation_id.is_none());
    assert!(result.started_at.is_none());
    assert!(result.result_due_at.is_none());
    assert!(result.retry_after_seconds.is_none());
    assert!(result.actions.contains(&SandboxLifecycleAction::Retire));
    assert_eq!(
        SandboxLifecycleResult::observe(&retained, None, at(960), true),
        Some(result)
    );
}

#[test]
fn failure_and_retirement_never_advertise_unavailable_retries_or_retirement() {
    let (sandbox, original) = failed_sandbox();
    let admission = SandboxLifecycleAdmission {
        operation_id: original.operation_id.clone().unwrap(),
        requested_at: original.started_at.clone().unwrap(),
    };
    let sandbox_bytes = serde_json::to_vec(&sandbox).unwrap();
    let mut sandbox: Sandbox = serde_json::from_slice(&sandbox_bytes).unwrap();
    let failed = SandboxLifecycleResult::observe_with_admission(
        &sandbox,
        None,
        Some(&admission),
        at(2),
        true,
    )
    .unwrap();
    assert_eq!(failed.operation_id, original.operation_id);
    assert_eq!(failed.started_at, original.started_at);
    assert_eq!(failed.result_due_at, original.result_due_at);
    assert_eq!(failed.state, SandboxLifecycleResultState::Failed);
    assert!(!failed.continuing);
    assert_eq!(failed.retry_after_seconds, None);
    assert_eq!(
        SandboxLifecycleResult::observe_with_admission(
            &sandbox,
            None,
            Some(&admission),
            at(600),
            true
        )
        .unwrap(),
        failed
    );
    sandbox.updated_at = sandbox.created_at.clone();
    sandbox.validate().unwrap();
    assert_eq!(
        SandboxLifecycleResult::observe_with_admission(
            &sandbox,
            None,
            Some(&admission),
            at(600),
            true
        )
        .unwrap(),
        failed
    );
    assert!(serde_json::to_value(&failed)
        .unwrap()
        .get("retry_after_seconds")
        .is_none());
    assert_eq!(
        SandboxLifecycleResult::observe_with_admission(
            &sandbox,
            None,
            Some(&admission),
            at(0),
            false
        )
        .unwrap()
        .actions,
        vec![SandboxLifecycleAction::Observe]
    );
    sandbox.begin_retirement(crate::rfc3339_at(at(3))).unwrap();
    assert!(SandboxLifecycleResult::observe_with_admission(
        &sandbox,
        None,
        Some(&admission),
        at(100),
        true
    )
    .is_none());
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
fn history_recovers_admission_and_rejects_ambiguous_or_completed_work() {
    use crate::sandbox::{SandboxEvent, SandboxEventKind, SandboxInterruption};
    let (mut sandbox, original) = failed_sandbox();
    let mut event = SandboxEvent {
        sandbox_id: sandbox.id.clone(),
        seq: 1,
        appended_at: lifecycle_test_support::NOW.into(),
        kind: SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Pausing,
        },
    };
    for status in [
        SandboxStatus::Pausing,
        SandboxStatus::Reviving,
        SandboxStatus::Rotating,
    ] {
        event.kind = SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: status,
        };
        let admission =
            SandboxLifecycleAdmission::from_history(&sandbox, &[event.clone()], &[]).unwrap();
        assert_eq!(Some(admission.operation_id), original.operation_id);
        assert_eq!(Some(admission.requested_at), original.started_at);
    }
    let settled = crate::sandbox::SandboxEvent {
        sandbox_id: sandbox.id.clone(),
        seq: 1,
        appended_at: event.appended_at.clone(),
        kind: crate::sandbox::SandboxEventKind::StatusChanged {
            from: SandboxStatus::Pausing,
            to: SandboxStatus::Paused,
        },
    };
    event.seq = 2;
    assert!(
        SandboxLifecycleAdmission::from_history(&sandbox, &[event.clone(), settled], &[]).is_some()
    );
    sandbox.lifecycle.interruption = Some(SandboxInterruption {
        cause: Default::default(),
        detected_at: crate::rfc3339_at(at(1)),
        memory_limit_mib: 1024,
    });
    assert!(SandboxLifecycleAdmission::from_history(&sandbox, &[event.clone()], &[]).is_none());
    sandbox.lifecycle.interruption = None;
    event.kind = SandboxEventKind::StatusChanged {
        from: SandboxStatus::Reviving,
        to: SandboxStatus::Active,
    };
    assert!(SandboxLifecycleAdmission::from_history(&sandbox, &[event], &[]).is_none());
    assert!(SandboxLifecycleAdmission::from_history(&sandbox, &[], &[]).is_none());
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
