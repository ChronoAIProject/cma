use super::*;

fn legacy() -> SandboxUnpublishedRecoverySnapshot {
    let mut held = stalled_revival();
    let fence = held
        .sandbox
        .work
        .claim(HOLDER.into(), NOW, END.into())
        .unwrap();
    held.sandbox
        .fail_lifecycle("startup failed".into(), HOLDER, fence, NOW.into())
        .unwrap();
    held.sandbox.lifecycle.failed_provision = None;
    held
}

fn observed(held: SandboxUnpublishedRecoverySnapshot) -> SandboxUnpublishedRecoveryRequest {
    let generation = held.sandbox.lifecycle.runtime_generation_high_water;
    let mut requested = request(held);
    requested.proof.boot_id = None;
    requested.proof.runtime_process_absent = true;
    requested.proof.observed_provision_generation = Some(generation);
    requested
}

#[test]
fn legacy_empty_allocation_uses_fresh_proof_without_inventing_old_operation() {
    let held = legacy();
    let requested = observed(held.clone());
    let mut after = apply_unpublished_runtime_recovery(&held, &requested)
        .unwrap()
        .sandbox;
    let proof = &after.lifecycle.unpublished_recovery.as_ref().unwrap().proof;
    assert_eq!(proof.runtime_generation, 1);
    assert_eq!(proof.observed_provision_generation, Some(2));
    assert!(proof.reserved_provision.is_none());
    assert_eq!(after.lifecycle.home, held.sandbox.lifecycle.home);
    assert_eq!(
        after.lifecycle.suspended_runtime,
        held.sandbox.lifecycle.suspended_runtime
    );
    held.sandbox
        .validate_runtime_generation_successor(&after)
        .unwrap();
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
fn legacy_recovery_refuses_missing_changed_or_automatic_authority() {
    for case in [
        "missing",
        "low",
        "high",
        "reserved",
        "boot",
        "process",
        "automatic",
        "agent",
        "changed",
    ] {
        let held = legacy();
        let mut requested = observed(held.clone());
        match case {
            "missing" => requested.proof.observed_provision_generation = None,
            "low" => requested.proof.observed_provision_generation = Some(1),
            "high" => requested.proof.observed_provision_generation = Some(3),
            "reserved" => {
                requested.proof.reserved_provision =
                    stalled_revival().sandbox.lifecycle.runtime_provision
            }
            "boot" => requested.proof.boot_id = Some("a".repeat(32)),
            "process" => requested.proof.runtime_process_absent = false,
            "automatic" => requested.automatic = true,
            "agent" => requested.snapshot.agents = snapshot(false).agents,
            "changed" => {
                requested
                    .snapshot
                    .sandbox
                    .lifecycle
                    .runtime_generation_high_water = 3
            }
            _ => unreachable!(),
        }
        assert!(
            apply_unpublished_runtime_recovery(&held, &requested).is_err(),
            "{case}"
        );
    }
}

#[test]
fn observed_allocation_is_not_valid_for_live_or_exact_reserved_sources() {
    for held in [snapshot(false), stalled_revival()] {
        let requested = observed(held.clone());
        assert!(apply_unpublished_runtime_recovery(&held, &requested).is_err());
    }
}

#[test]
fn legacy_process_proof_is_immutable_after_admission() {
    let held = legacy();
    let admitted = apply_unpublished_runtime_recovery(&held, &observed(held.clone()))
        .unwrap()
        .sandbox;
    for generation in [None, Some(1), Some(3)] {
        let mut altered = admitted.clone();
        altered
            .lifecycle
            .unpublished_recovery
            .as_mut()
            .unwrap()
            .proof
            .observed_provision_generation = generation;
        assert!(admitted
            .validate_unpublished_recovery_save(&altered)
            .is_err());
        assert!(admitted
            .validate_runtime_generation_successor(&altered)
            .is_err());
    }
}
