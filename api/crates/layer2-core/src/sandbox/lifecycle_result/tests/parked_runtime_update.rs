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
