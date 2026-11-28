use super::*;
use crate::sandbox::runtime_update_lifecycle_tests::{admit, pause, provision, step};
use crate::sandbox::runtime_update_test_support::native;
use crate::sandbox::{SandboxWorkStep, TypedFailure};

const HOLDER: &str = "runtime-update-test";
const NOW: &str = lifecycle_test_support::NOW;

fn parked(revive: bool) -> Sandbox {
    let mut value = native();
    if revive {
        pause(&mut value);
        value
            .begin_revive_with_managed_runtime(
                SandboxReviveCause::ExplicitRequest,
                Some("image-v2"),
                NOW.into(),
            )
            .unwrap();
    } else {
        admit(&mut value);
        step(&mut value, |v, f| {
            v.complete_lifecycle_persistent_pause(HOLDER, f, NOW.into())
        });
        step(&mut value, |v, f| {
            v.complete_lifecycle_agent_pause(HOLDER, f, NOW.into())
        });
        step(&mut value, |v, f| {
            v.complete_lifecycle_runtime_suspension(HOLDER, f, NOW.into())
        });
    }
    provision(&mut value);
    step(&mut value, |v, f| {
        v.complete_lifecycle_credential_delivery(HOLDER, f, NOW.into())
    });
    step(&mut value, |v, f| {
        v.complete_lifecycle_home_restore(HOLDER, f, NOW.into())
    });
    let before = value.clone();
    let fence = value
        .work
        .claim(HOLDER.into(), NOW, crate::rfc3339_at(at(60)))
        .unwrap();
    value
        .park_failed_runtime_update(
            TypedFailure {
                class: "terminal_validation".into(),
                detail: "Synthetic refusal".into(),
            },
            HOLDER,
            fence,
            NOW.into(),
        )
        .unwrap();
    assert_eq!(value.lifecycle.operation, before.lifecycle.operation);
    assert_eq!(value.lifecycle.home, before.lifecycle.home);
    assert_eq!(value.runtime, before.runtime);
    assert_eq!(value.work.step, SandboxWorkStep::None);
    assert!(value.work.lease.is_none());
    value.validate().unwrap();
    value
}

#[test]
fn parked_runtime_update_result_preserves_admission_and_storage_for_rotation_and_revive() {
    for revive in [false, true] {
        let sandbox = parked(revive);
        let bytes = serde_json::to_vec(&sandbox).unwrap();
        let admission = sandbox.lifecycle.operation.as_ref().unwrap().authority();
        for elapsed in [0, 89, 90, 960] {
            let result =
                SandboxLifecycleResult::observe(&sandbox, None, at(elapsed), false).unwrap();
            assert_eq!(result.state, SandboxLifecycleResultState::Failed);
            assert_eq!(result.code, Some("runtime_update_parked"));
            assert!(!result.continuing);
            assert!(result.retry_after_seconds.is_none());
            assert_eq!(
                result.operation_id.as_deref(),
                Some(admission.operation_id.as_str())
            );
            assert_eq!(
                result.started_at.as_deref(),
                Some(admission.requested_at.as_str())
            );
            assert_eq!(result.result_due_at, Some(crate::rfc3339_at(at(90))));
            assert_eq!(result.actions, vec![SandboxLifecycleAction::Observe]);
        }
        assert_eq!(serde_json::to_vec(&sandbox).unwrap(), bytes);
        let restored: Sandbox = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored, sandbox);
        let old: predecessor::PredecessorLifecycle =
            serde_json::from_value(serde_json::to_value(&sandbox.lifecycle).unwrap()).unwrap();
        assert_eq!(
            serde_json::to_value(old).unwrap(),
            serde_json::to_value(&sandbox.lifecycle).unwrap()
        );
    }
}

#[test]
fn parked_runtime_update_retry_returns_to_normal_observation_without_retargeting() {
    for revive in [false, true] {
        let mut sandbox = parked(revive);
        let operation = sandbox.lifecycle.operation.clone();
        assert!(sandbox
            .request_runtime_update("image-v2".into(), false, NOW.into())
            .unwrap());
        for elapsed in [89, 90] {
            let result =
                SandboxLifecycleResult::observe(&sandbox, None, at(elapsed), false).unwrap();
            assert_eq!(
                result.state,
                if elapsed < 90 {
                    SandboxLifecycleResultState::InProgress
                } else {
                    SandboxLifecycleResultState::RecoverablePending
                }
            );
            assert!(result.continuing);
            assert_eq!(result.retry_after_seconds, Some(5));
        }
        assert_eq!(sandbox.lifecycle.operation, operation);
        assert_eq!(
            sandbox.runtime_update_request().unwrap().target_version,
            "image-v2"
        );
    }
}

#[test]
fn parked_runtime_update_requires_current_admission_and_stopped_work() {
    let changes: [fn(&mut Sandbox); 9] = [
        |s| s.lifecycle.runtime_update.as_mut().unwrap().request = None,
        |s| {
            s.lifecycle
                .runtime_update
                .as_mut()
                .unwrap()
                .request
                .as_mut()
                .unwrap()
                .failure = None
        },
        |s| {
            s.lifecycle
                .runtime_update
                .as_mut()
                .unwrap()
                .request
                .as_mut()
                .unwrap()
                .started_at = None
        },
        |s| {
            s.lifecycle
                .runtime_update
                .as_mut()
                .unwrap()
                .request
                .as_mut()
                .unwrap()
                .completed_at = Some(NOW.into())
        },
        |s| {
            s.lifecycle
                .runtime_update
                .as_mut()
                .unwrap()
                .request
                .as_mut()
                .unwrap()
                .target_version = "different".into()
        },
        |s| {
            s.lifecycle
                .runtime_update
                .as_mut()
                .unwrap()
                .request
                .as_mut()
                .unwrap()
                .started_at = Some(crate::rfc3339_at(at(1)))
        },
        |s| {
            s.work
                .schedule(SandboxWorkStep::RunLifecycleOperation, NOW.into())
        },
        |s| {
            s.work
                .schedule(SandboxWorkStep::RunLifecycleOperation, NOW.into());
            s.work
                .claim(HOLDER.into(), NOW, crate::rfc3339_at(at(60)))
                .unwrap();
            s.work.step = SandboxWorkStep::None;
        },
        |s| s.status = SandboxStatus::Active,
    ];
    for (i, change) in changes.iter().enumerate() {
        let mut sandbox = parked(false);
        change(&mut sandbox);
        assert_ne!(
            SandboxLifecycleResult::observe(&sandbox, None, at(960), false).and_then(|r| r.code),
            Some("runtime_update_parked"),
            "case {i}"
        );
    }
    let mut sandbox = parked(false);
    sandbox.lifecycle.operation = None;
    assert!(SandboxLifecycleResult::observe(&sandbox, None, at(960), false).is_none());
    let mut sandbox = parked(true);
    sandbox
        .lifecycle
        .runtime_update
        .as_mut()
        .unwrap()
        .request
        .as_mut()
        .unwrap()
        .started_at = Some(crate::rfc3339_at(at(0) - Duration::from_secs(1)));
    assert_eq!(
        SandboxLifecycleResult::observe(&sandbox, None, at(960), false)
            .unwrap()
            .code,
        Some("runtime_update_parked")
    );
}

#[test]
fn parked_runtime_update_keeps_retirement_and_aggregate_failure_precedence() {
    let mut sandbox = parked(false);
    let result = SandboxLifecycleResult::observe(&sandbox, None, at(960), true).unwrap();
    assert!(result.actions.contains(&SandboxLifecycleAction::Retire));
    sandbox.begin_retirement(NOW.into()).unwrap();
    let result = SandboxLifecycleResult::observe(&sandbox, Some(NOW), at(960), true).unwrap();
    assert_eq!(result.code, Some("lifecycle_wait_exceeded"));
    assert!(result.continuing);
    let mut sandbox = parked(false);
    sandbox.status = SandboxStatus::Failed;
    assert_eq!(
        SandboxLifecycleResult::observe(&sandbox, None, at(960), false)
            .unwrap()
            .code,
        Some("lifecycle_failed_admission_unavailable")
    );
}

#[test]
fn runtime_loss_rotation_syncs_start_and_recognizes_legacy_parked_rows() {
    let lost_at = crate::rfc3339_at(at(60));
    for retained_home in [false, true] {
        let mut before = if retained_home {
            let mut value = native();
            admit(&mut value);
            value
        } else {
            let mut value = lifecycle_test_support::sandbox();
            value.lifecycle.checkpoints.latest = Some(lifecycle_test_support::checkpoint(1, NOW));
            value
                .begin_image_rotation(
                    "image-v2".into(),
                    crate::sandbox::SandboxRotationIdentity::capture(&value, &[]).unwrap(),
                    NOW.into(),
                )
                .unwrap();
            value.lifecycle.runtime_update = native_rotation_request();
            value
        };
        before.validate().unwrap();
        for enabled in [false, true] {
            let mut recovered = before.clone();
            recovered
                .recover_runtime_loss_with_policy(&[], retained_home, lost_at.clone(), enabled)
                .unwrap();
            assert_eq!(
                recovered
                    .runtime_update_request()
                    .unwrap()
                    .started_at
                    .as_deref(),
                Some(if enabled { lost_at.as_str() } else { NOW })
            );
            assert_eq!(recovered.lifecycle.home, before.lifecycle.home);
            assert_eq!(
                recovered.lifecycle.checkpoints,
                before.lifecycle.checkpoints
            );
            assert_eq!(
                recovered
                    .lifecycle
                    .operation
                    .as_ref()
                    .unwrap()
                    .authority()
                    .requested_at,
                lost_at
            );
        }

        // Simulate a pre-flag row recovered by the store from the old start timestamp.
        before
            .recover_runtime_loss_with_policy(&[], retained_home, lost_at.clone(), false)
            .unwrap();
        before.lifecycle.interruption = Some(crate::sandbox::SandboxInterruption {
            cause: crate::sandbox::SandboxInterruptionCause::RuntimeUnavailable,
            detected_at: lost_at.clone(),
            memory_limit_mib: before.profile.content.memory_mib,
        });
        let fence = before
            .work
            .claim(HOLDER.into(), &lost_at, crate::rfc3339_at(at(180)))
            .unwrap();
        before
            .park_failed_runtime_update(
                TypedFailure {
                    class: "terminal_validation".into(),
                    detail: "Synthetic refusal".into(),
                },
                HOLDER,
                fence,
                lost_at.clone(),
            )
            .unwrap();
        let prior = SandboxLifecycleResult::observe(&before, None, at(960), true).unwrap();
        assert!(prior.continuing);
        let enabled =
            SandboxLifecycleResult::observe_with_policy(&before, None, None, at(960), true, true)
                .unwrap();
        assert_eq!(enabled.code, Some("runtime_update_parked"));
        assert!(enabled.actions.contains(&SandboxLifecycleAction::Retire));
        before.lifecycle.interruption = None;
        let unrelated =
            SandboxLifecycleResult::observe_with_policy(&before, None, None, at(960), true, true)
                .unwrap();
        assert_eq!(unrelated.code, Some("lifecycle_wait_exceeded"));
    }
}

fn native_rotation_request() -> Option<crate::sandbox::SandboxRuntimeUpdateState> {
    let mut value = native();
    admit(&mut value);
    value.lifecycle.runtime_update
}

#[test]
fn only_a_stalled_rotation_reports_the_new_owner_retirement_projection() {
    let mut value = native();
    admit(&mut value);
    value.work.attempts = 3;
    value.work.ready_at = crate::rfc3339_at(at(1800));
    let project = |row: &Sandbox, owner, enabled| {
        SandboxLifecycleResult::observe_with_policy(row, None, None, at(960), owner, enabled)
            .unwrap()
    };
    assert_eq!(
        project(&value, true, false).code,
        Some("lifecycle_wait_exceeded")
    );
    let result = project(&value, true, true);
    assert_eq!(result.code, Some("runtime_update_stalled"));
    assert!(result.actions.contains(&SandboxLifecycleAction::Retire));
    assert!(!project(&value, false, true)
        .actions
        .contains(&SandboxLifecycleAction::Retire));

    let due_at = value.work.ready_at.clone();
    value
        .work
        .claim(HOLDER.into(), &due_at, crate::rfc3339_at(at(1900)))
        .unwrap();
    assert_eq!(
        project(&value, true, true).code,
        Some("lifecycle_wait_exceeded")
    );
}
