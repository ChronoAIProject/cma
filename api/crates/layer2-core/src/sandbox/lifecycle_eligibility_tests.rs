use std::sync::Arc;

use super::lifecycle_test_support::{checkpoint, sandbox, NOW};
use super::store::SandboxStore;
use super::*;
use crate::profiles::store::InMemoryProfileStore;
use crate::{Authorization, Caller, Capability, SubjectId};

const LATER: &str = "2026-09-05T00:31:00Z";
const EXPIRES: &str = "2026-09-05T00:36:00Z";

fn active() -> Sandbox {
    let mut value = sandbox();
    value.runtime.as_mut().unwrap().provision_generation = Some(1);
    value.lifecycle.runtime_generation_high_water = 1;
    value
}

fn claim_capture(value: &mut Sandbox, at: &str) -> u64 {
    value
        .claim_checkpoint_capture("capture-worker".into(), at.into(), EXPIRES.into())
        .unwrap();
    value
        .lifecycle
        .checkpoint_capture
        .as_ref()
        .unwrap()
        .work
        .lease
        .as_ref()
        .unwrap()
        .fence
}

#[test]
fn lifecycle_eligibility_matches_transition_guards_for_status_work_and_claims() {
    for status in [
        SandboxStatus::Provisioning,
        SandboxStatus::Installing,
        SandboxStatus::Active,
        SandboxStatus::Pausing,
        SandboxStatus::Paused,
        SandboxStatus::Reviving,
        SandboxStatus::CredentialRotating,
        SandboxStatus::Rotating,
        SandboxStatus::Failed,
        SandboxStatus::Retiring,
        SandboxStatus::Retired,
    ] {
        for step in [
            SandboxWorkStep::None,
            SandboxWorkStep::WaitForCredentialRotation,
            SandboxWorkStep::Activate,
            SandboxWorkStep::RunLifecycleOperation,
        ] {
            for claimed in [false, true] {
                let mut value = active();
                value.status = status;
                value.lifecycle.checkpoints.latest = Some(checkpoint(1, NOW));
                value.work.step = step;
                if claimed {
                    value.work.lease = Some(SandboxWorkLease {
                        holder: "lifecycle-eligibility-worker".into(),
                        fence: 1,
                        expires_at: EXPIRES.into(),
                    });
                    value.work.next_fence = 2;
                }
                let expected_pause = status == SandboxStatus::Active
                    && !claimed
                    && matches!(
                        step,
                        SandboxWorkStep::None | SandboxWorkStep::WaitForCredentialRotation
                    );
                let expected_revive =
                    status == SandboxStatus::Paused && !claimed && step == SandboxWorkStep::None;
                assert_eq!(value.pause_eligibility().is_ok(), expected_pause);
                assert_eq!(value.revive_eligibility().is_ok(), expected_revive);
                let mut paused = value.clone();
                assert_eq!(
                    paused.begin_pause(LATER.into(), None).is_ok(),
                    expected_pause
                );
                if !expected_pause {
                    assert_eq!(paused, value);
                }
                let mut revived = value.clone();
                assert_eq!(
                    revived
                        .begin_revive(SandboxReviveCause::ExplicitRequest, LATER.into())
                        .is_ok(),
                    expected_revive
                );
                if !expected_revive {
                    assert_eq!(revived, value);
                }
            }
        }
    }
}

#[test]
fn pause_remains_ineligible_until_checkpoint_writer_release_is_recorded() {
    let mut value = active();
    value.begin_checkpoint_capture(NOW.into()).unwrap();
    let fence = claim_capture(&mut value, NOW);
    for published in [false, true] {
        if published {
            value
                .publish_captured_checkpoint(
                    checkpoint(1, NOW),
                    "capture-worker",
                    fence,
                    LATER.into(),
                )
                .unwrap();
        }
        assert_eq!(
            value.pause_eligibility(),
            Err(SandboxLifecycleBlocker::CheckpointInProgress)
        );
        let before = value.clone();
        assert!(value.begin_pause(LATER.into(), None).is_err());
        assert_eq!(value, before);
    }
    let fence = claim_capture(&mut value, LATER);
    value
        .complete_checkpoint_writer_resume("capture-worker", fence, LATER.into())
        .unwrap();
    assert_eq!(value.pause_eligibility(), Ok(()));
    value.begin_pause(LATER.into(), None).unwrap();
}

#[tokio::test]
async fn busy_lifecycle_refusal_is_typed_preserves_state_and_conceals_foreign_ids() {
    for case in 0..4 {
        let mut value = active();
        let (revive, reason) = match case {
            0 => {
                value.begin_checkpoint_capture(NOW.into()).unwrap();
                (false, SandboxLifecycleBlocker::CheckpointInProgress)
            }
            1 => {
                value.work.step = SandboxWorkStep::WaitForCredentialRotation;
                value
                    .work
                    .claim("credential-worker".into(), NOW, EXPIRES.into())
                    .unwrap();
                (false, SandboxLifecycleBlocker::WorkInProgress)
            }
            2 => {
                value.work.step = SandboxWorkStep::Activate;
                (false, SandboxLifecycleBlocker::WorkInProgress)
            }
            _ => {
                value.status = SandboxStatus::Paused;
                value.runtime = None;
                value.lifecycle.checkpoints.latest = Some(checkpoint(1, NOW));
                value.work.step = SandboxWorkStep::RunLifecycleOperation;
                (true, SandboxLifecycleBlocker::WorkInProgress)
            }
        };
        value.revision = 1;
        let store = Arc::new(InMemorySandboxStore::default());
        store.create_sandbox(value.clone()).await.unwrap();
        let service = SandboxService::new(
            store.clone(),
            Arc::new(InMemoryProfileStore::default()),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        );
        for (subject, expected) in [
            (
                "owner",
                SandboxLifecycleCommandError::SandboxBusy {
                    status: value.status,
                    reason,
                },
            ),
            ("foreign", SandboxLifecycleCommandError::NotFound),
        ] {
            let caller = Caller::new(
                SubjectId(subject.into()),
                Authorization::new([Capability::UseOwnSandboxes]),
            );
            let result = if revive {
                service.revive(&caller, &value.id).await
            } else {
                service.pause(&caller, &value.id).await
            };
            assert_eq!(result, Err(expected));
            assert_eq!(
                store.get_sandbox(&value.id).await.unwrap(),
                Some(value.clone())
            );
        }
        assert_eq!(
            SandboxLifecycleCommandError::SandboxBusy {
                status: value.status,
                reason
            }
            .code(),
            "sandbox_lifecycle_busy"
        );
    }
}
