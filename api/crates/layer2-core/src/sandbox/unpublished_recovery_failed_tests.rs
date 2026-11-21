use super::*;

fn failed_revival() -> SandboxUnpublishedRecoverySnapshot {
    fail(stalled_revival())
}

fn fail(mut held: SandboxUnpublishedRecoverySnapshot) -> SandboxUnpublishedRecoverySnapshot {
    let fence = held
        .sandbox
        .work
        .claim(HOLDER.into(), NOW, END.into())
        .unwrap();
    held.sandbox
        .fail_lifecycle("runtime startup failed".into(), HOLDER, fence, NOW.into())
        .unwrap();
    held
}

#[test]
fn terminal_failed_allocation_retains_exact_source_and_can_recover_above_reserved_generation() {
    let original = stalled_revival();
    let held = fail(original.clone());
    held.validate().unwrap();
    assert_eq!(held.sandbox.status, SandboxStatus::Failed);
    assert!(held.sandbox.lifecycle.operation.is_none());
    assert!(held.sandbox.lifecycle.runtime_provision.is_none());
    assert_eq!(held.sandbox.work.step, SandboxWorkStep::None);
    let evidence = held.sandbox.lifecycle.failed_provision.as_ref().unwrap();
    assert_eq!(
        Some(&evidence.reservation),
        original.sandbox.lifecycle.runtime_provision.as_ref()
    );
    assert_eq!(
        Some(&evidence.operation),
        original.sandbox.lifecycle.operation.as_ref()
    );
    assert_eq!(
        Some(&evidence.source),
        original.sandbox.lifecycle.suspended_runtime.as_deref()
    );
    assert_eq!(held.sandbox.lifecycle.home, original.sandbox.lifecycle.home);
    let encoded = serde_json::to_vec(&held.sandbox).unwrap();
    let decoded: Sandbox = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, held.sandbox);
    decoded.validate().unwrap();

    let mut requested = request(held.clone());
    requested.proof.boot_id = None;
    requested.proof.runtime_process_absent = true;
    requested.proof.reserved_provision = Some(evidence.reservation.clone());
    let mut after = apply_unpublished_runtime_recovery(&held, &requested)
        .unwrap()
        .sandbox;
    assert!(after.lifecycle.failed_provision.is_none());
    assert_eq!(after.lifecycle.home, held.sandbox.lifecycle.home);
    assert_eq!(
        after.lifecycle.suspended_runtime,
        held.sandbox.lifecycle.suspended_runtime
    );
    held.sandbox
        .validate_runtime_generation_successor(&after)
        .unwrap();
    assert_eq!(after.retained_home_recovery().unwrap().source_generation, 1);
    let receipt = quiescence(&after);
    let fence = after.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    after
        .complete_unpublished_runtime_quiescence(receipt, HOLDER, fence, NOW.into())
        .unwrap();
    let source = after.retained_home_recovery().unwrap().clone();
    let fence = after.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    after
        .complete_lifecycle_home_verification(&source, HOLDER, fence, NOW.into())
        .unwrap();
    let fence = after.work.claim(HOLDER.into(), NOW, END.into()).unwrap();
    assert_eq!(
        after
            .reserve_lifecycle_runtime_generation(2, HOLDER, fence, NOW.into())
            .unwrap(),
        3
    );
}

#[test]
fn failed_allocation_never_recovers_from_high_water_without_exact_retained_evidence() {
    let mut legacy = failed_revival();
    legacy.sandbox.lifecycle.failed_provision = None;
    legacy.sandbox.validate().unwrap();
    assert_eq!(legacy.sandbox.lifecycle.runtime_generation_high_water, 2);
    assert!(legacy.sandbox.unpublished_recovery_candidate());
    legacy.validate().unwrap();
    // Selection permits a trusted fresh inspection, never admission from the
    // ceiling alone or a recreated historical reservation.
    assert!(apply_unpublished_runtime_recovery(&legacy, &request(legacy.clone())).is_err());
}

#[test]
fn failed_allocation_refuses_changed_source_home_authority_or_children() {
    for case in [
        "source",
        "home",
        "operation",
        "generation",
        "high_water",
        "owner",
        "agent",
        "lease",
        "proof",
    ] {
        let mut held = failed_revival();
        match case {
            "source" => held
                .sandbox
                .lifecycle
                .suspended_runtime
                .as_mut()
                .unwrap()
                .runtime_id
                .push('x'),
            "home" => held
                .sandbox
                .lifecycle
                .failed_provision
                .as_mut()
                .unwrap()
                .home
                .home_identity
                .push('x'),
            "operation" => held
                .sandbox
                .lifecycle
                .failed_provision
                .as_mut()
                .unwrap()
                .reservation
                .operation_id
                .push('x'),
            "generation" => {
                held.sandbox
                    .lifecycle
                    .failed_provision
                    .as_mut()
                    .unwrap()
                    .reservation
                    .generation += 1
            }
            "high_water" => held.sandbox.lifecycle.runtime_generation_high_water += 1,
            "owner" => held.sandbox.owner = crate::SubjectId("different-owner".into()),
            "agent" => held
                .agents
                .push(agent("agt_unexpected", "native-original", 0)),
            "lease" => {
                held.sandbox.work.lease = Some(crate::sandbox::SandboxWorkLease {
                    holder: HOLDER.into(),
                    fence: 2,
                    expires_at: END.into(),
                });
            }
            "proof" => {
                let mut requested = request(held.clone());
                requested.proof.runtime_process_absent = false;
                assert!(apply_unpublished_runtime_recovery(&held, &requested).is_err());
                continue;
            }
            _ => unreachable!(),
        }
        assert!(held.validate().is_err(), "{case}");
    }
}

#[test]
fn generic_saves_cannot_erase_or_inject_failed_allocation_evidence() {
    let held = failed_revival().sandbox;
    let mut cleared = held.clone();
    cleared.lifecycle.failed_provision = None;
    assert!(held.validate_unpublished_recovery_save(&cleared).is_err());
    assert!(held
        .validate_runtime_generation_successor(&cleared)
        .is_err());
    assert!(cleared.validate_unpublished_recovery_save(&held).is_err());
    assert!(cleared
        .validate_runtime_generation_successor(&held)
        .is_err());
    let mut retired = held.clone();
    retired.begin_retirement(NOW.into()).unwrap();
    assert!(retired.lifecycle.failed_provision.is_none());
    held.validate_unpublished_recovery_save(&retired).unwrap();
    held.validate_runtime_generation_successor(&retired)
        .unwrap();
}
