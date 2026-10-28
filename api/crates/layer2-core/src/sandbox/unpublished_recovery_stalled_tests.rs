use super::*;
use crate::sandbox::{QueuedAgentMessage, SandboxReviveCause, SandboxRevivePhase};

fn stalled_reopen(revive: bool) -> SandboxUnpublishedRecoverySnapshot {
    let mut held = snapshot(true);
    if revive {
        held.sandbox
            .lifecycle
            .start_revive(
                &held.sandbox.id,
                NOW.into(),
                SandboxReviveCause::RuntimeLost,
            )
            .unwrap();
        held.sandbox.status = SandboxStatus::Reviving;
        let Some(SandboxLifecycleOperation::Revive { phase, .. }) =
            &mut held.sandbox.lifecycle.operation
        else {
            panic!()
        };
        *phase = SandboxRevivePhase::ConversationReopen;
    }
    let update = held
        .sandbox
        .lifecycle
        .runtime_update
        .as_mut()
        .unwrap()
        .request
        .as_mut()
        .unwrap();
    update.failure = None;
    if revive {
        update.started_at = None;
    }
    held.sandbox.lifecycle.runtime_generation_high_water = 16;
    held.sandbox.runtime.as_mut().unwrap().provision_generation = Some(16);
    held.sandbox
        .lifecycle
        .home_recovery
        .as_mut()
        .unwrap()
        .source_generation = 15;
    held.agents[0].conversation.runtime_generation = Some(15);
    held.agents[0].work.receipts.clear();
    held.agents[0]
        .conversation
        .queued_messages
        .push(QueuedAgentMessage::new(1, "retained owner input".into(), NOW.into()).unwrap());
    held.sandbox
        .work
        .schedule(SandboxWorkStep::RunLifecycleOperation, NOW.into());
    let fence = held
        .sandbox
        .work
        .claim(HOLDER.into(), NOW, END.into())
        .unwrap();
    held.sandbox
        .retry_lifecycle(END.into(), HOLDER, fence, NOW.into())
        .unwrap();
    held.sandbox.work.attempts = 4036;
    held.sandbox.validate().unwrap();
    held
}

#[test]
fn retried_reopen_can_be_explicitly_recovered_with_original_home_and_queued_input() {
    for revive in [false, true] {
        let held = stalled_reopen(revive);
        assert!(held.sandbox.unpublished_recovery_candidate());
        assert!(held
            .sandbox
            .runtime_update_request()
            .unwrap()
            .failure
            .is_none());
        let requested = request(held.clone());
        let after = apply_unpublished_runtime_recovery(&held, &requested)
            .unwrap()
            .sandbox;
        assert_eq!(after.status, SandboxStatus::Rotating);
        assert_eq!(after.lifecycle.home, held.sandbox.lifecycle.home);
        assert_eq!(
            after.lifecycle.suspended_runtime.as_deref(),
            held.sandbox.runtime.as_ref()
        );
        assert_eq!(
            after.retained_home_recovery().unwrap().source_generation,
            16
        );
        assert_eq!(after.lifecycle.runtime_generation_high_water, 16);
        assert_eq!(after.admitted_runtime_update_target(), Some("image-v3"));
        assert_ne!(
            after
                .lifecycle
                .operation
                .as_ref()
                .unwrap()
                .authority()
                .operation_id,
            held.sandbox
                .lifecycle
                .operation
                .as_ref()
                .unwrap()
                .authority()
                .operation_id
        );
        assert_eq!(requested.snapshot.agents, held.agents);
        assert!(after.require_unpublished_runtime_quiescence().is_err());
    }
}

#[test]
fn stalled_reopen_requires_prior_retry_exact_phase_and_no_worker_authority() {
    for revive in [false, true] {
        for case in 0..8 {
            let mut held = stalled_reopen(revive);
            match case {
                0 => held.sandbox.work.attempts = 0,
                1 => {
                    held.sandbox
                        .work
                        .claim(HOLDER.into(), END, "2026-09-05T02:00:00Z".into())
                        .unwrap();
                }
                2 => held.sandbox.work.step = SandboxWorkStep::None,
                3 => held.sandbox.runtime = None,
                4 | 5 => match held.sandbox.lifecycle.operation.as_mut().unwrap() {
                    SandboxLifecycleOperation::Revive { phase, .. } => {
                        *phase = if case == 4 {
                            SandboxRevivePhase::HomeRestore
                        } else {
                            SandboxRevivePhase::QueuedMessageDelivery
                        }
                    }
                    SandboxLifecycleOperation::ImageRotation { phase, .. } => {
                        *phase = if case == 4 {
                            SandboxImageRotationPhase::HomeRestore
                        } else {
                            SandboxImageRotationPhase::QueuedMessageDelivery
                        }
                    }
                    _ => unreachable!(),
                },
                6 => held.agents[0].conversation.runtime_generation = Some(16),
                7 => held.agents[0].conversation.active_turn = Some(8),
                _ => unreachable!(),
            }
            assert!(held.validate().is_err(), "revive {revive}, case {case}");
        }
    }
}

#[test]
fn automatic_stalled_recovery_preserves_queued_or_started_target_and_incident_bound() {
    for revive in [false, true] {
        let held = stalled_reopen(revive);
        let mut requested = request(held.clone());
        requested.automatic = true;
        for target in [
            "image-v3",
            held.sandbox
                .runtime
                .as_ref()
                .unwrap()
                .runtime_version
                .as_str(),
        ] {
            requested.target_runtime_version = target.into();
            assert!(
                apply_unpublished_runtime_recovery(&held, &requested).is_err(),
                "revive {revive}, target {target}"
            );
        }
        requested.target_runtime_version = "image-v2".into();
        let after = apply_unpublished_runtime_recovery(&held, &requested)
            .unwrap()
            .sandbox;
        assert_eq!(after.admitted_runtime_update_target(), Some("image-v2"));
        assert!(after.runtime_update_request().unwrap().automatic);
        // A prior unpublished-recovery incident is always an ImageRotation.
        if !revive {
            let mut attempted = held.clone();
            attempted.sandbox.lifecycle.unpublished_recovery_attempted = true;
            attempted.validate().unwrap();
            requested.snapshot = attempted.clone();
            assert!(apply_unpublished_runtime_recovery(&attempted, &requested).is_err());
            requested.automatic = false;
            apply_unpublished_runtime_recovery(&attempted, &requested).unwrap();
        }
    }
}

#[test]
fn stalled_reopen_never_bypasses_native_source_and_history_proof() {
    let held = stalled_reopen(true);
    for case in 0..4 {
        let mut requested = request(held.clone());
        match case {
            0 => requested.proof.boot_id = None,
            1 => requested.proof.conversations.clear(),
            2 => requested.proof.runtime_generation = 15,
            3 => requested.proof.requires_native_quiescence = false,
            _ => unreachable!(),
        }
        assert!(
            apply_unpublished_runtime_recovery(&held, &requested).is_err(),
            "case {case}"
        );
    }
}

#[test]
fn reopen_worker_claim_after_native_inspection_invalidates_recovery_snapshot() {
    let held = stalled_reopen(true);
    let requested = request(held.clone());
    let mut claimed = held;
    claimed
        .sandbox
        .work
        .claim(HOLDER.into(), END, "2026-09-05T02:00:00Z".into())
        .unwrap();
    assert!(matches!(
        apply_unpublished_runtime_recovery(&claimed, &requested),
        Err(SandboxStoreError::Conflict)
    ));
}
