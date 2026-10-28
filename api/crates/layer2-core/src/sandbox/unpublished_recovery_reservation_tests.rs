use super::*;

fn reserved_request() -> SandboxUnpublishedRecoveryRequest {
    let held = stalled_revival();
    let mut requested = request(held.clone());
    requested.proof.boot_id = None;
    requested.proof.runtime_process_absent = true;
    requested.proof.reserved_provision = held.recovery_provision_reservation().unwrap().cloned();
    requested
}

#[test]
fn reserved_recovery_preserves_original_binding_and_carries_exact_old_operation() {
    let requested = reserved_request();
    let held = &requested.snapshot;
    let reservation = held.sandbox.lifecycle.runtime_provision.clone().unwrap();
    let after = apply_unpublished_runtime_recovery(held, &requested)
        .unwrap()
        .sandbox;
    assert_eq!(
        after.lifecycle.suspended_runtime,
        held.sandbox.lifecycle.suspended_runtime
    );
    assert_eq!(after.lifecycle.home, held.sandbox.lifecycle.home);
    assert_eq!(after.retained_home_recovery().unwrap().source_generation, 1);
    assert_eq!(after.lifecycle.runtime_generation_high_water, 2);
    assert!(after.lifecycle.runtime_provision.is_none());
    let state = after.lifecycle.unpublished_recovery.as_ref().unwrap();
    assert_eq!(state.proof.runtime_generation, 1);
    assert_eq!(state.proof.reserved_provision.as_ref(), Some(&reservation));
    assert_ne!(state.operation_id, reservation.operation_id);
    after.validate().unwrap();
}

#[test]
fn reservation_requires_exact_operation_generation_high_water_and_source() {
    for change in [
        "missing",
        "operation",
        "generation",
        "high-water",
        "source",
        "home",
        "boot",
        "running",
    ] {
        let mut requested = reserved_request();
        match change {
            "missing" => requested.snapshot.sandbox.lifecycle.runtime_provision = None,
            "operation" => requested
                .proof
                .reserved_provision
                .as_mut()
                .unwrap()
                .operation_id
                .push('0'),
            "generation" => {
                requested
                    .proof
                    .reserved_provision
                    .as_mut()
                    .unwrap()
                    .generation += 1
            }
            "high-water" => {
                requested
                    .snapshot
                    .sandbox
                    .lifecycle
                    .runtime_generation_high_water += 1
            }
            "source" => requested.proof.runtime_generation += 1,
            "home" => requested.proof.home_identity = "changed".into(),
            "boot" => requested.proof.boot_id = Some("a".repeat(32)),
            "running" => requested.proof.runtime_process_absent = false,
            _ => unreachable!(),
        }
        assert!(
            apply_unpublished_runtime_recovery(&requested.snapshot, &requested).is_err(),
            "{change}"
        );
    }
    let reserved = reserved_request().proof.reserved_provision;
    let mut ordinary = request(snapshot(false));
    ordinary.proof.reserved_provision = reserved;
    assert!(apply_unpublished_runtime_recovery(&ordinary.snapshot, &ordinary).is_err());
}

#[test]
fn reserved_recovery_rejects_changed_transaction_snapshot_and_generic_proof_edits() {
    let requested = reserved_request();
    let mut current = requested.snapshot.clone();
    current.sandbox.revision += 1;
    assert!(matches!(
        apply_unpublished_runtime_recovery(&current, &requested),
        Err(SandboxStoreError::Conflict)
    ));
    let admitted = apply_unpublished_runtime_recovery(&requested.snapshot, &requested)
        .unwrap()
        .sandbox;
    let mut after = admitted.clone();
    after
        .lifecycle
        .unpublished_recovery
        .as_mut()
        .unwrap()
        .proof
        .reserved_provision
        .as_mut()
        .unwrap()
        .operation_id
        .push('0');
    assert!(admitted.validate_unpublished_recovery_save(&after).is_err());
    assert!(admitted
        .validate_runtime_generation_successor(&after)
        .is_err());
    for change in ["operation", "floor", "source"] {
        let mut after = admitted.clone();
        match change {
            "operation" => {
                after
                    .lifecycle
                    .unpublished_recovery
                    .as_mut()
                    .unwrap()
                    .proof
                    .reserved_provision
                    .as_mut()
                    .unwrap()
                    .operation_id = "foreign".into()
            }
            "floor" => after.lifecycle.runtime_generation_high_water = 1,
            "source" => {
                after
                    .lifecycle
                    .unpublished_recovery
                    .as_mut()
                    .unwrap()
                    .proof
                    .reserved_provision
                    .as_mut()
                    .unwrap()
                    .generation = 1
            }
            _ => unreachable!(),
        }
        assert!(after.validate().is_err(), "{change}");
    }
}

#[test]
fn old_proofs_without_reservations_round_trip_unchanged() {
    let old = request(snapshot(false)).proof;
    let encoded = serde_json::to_value(&old).unwrap();
    assert!(encoded.get("reserved_provision").is_none());
    assert_eq!(
        serde_json::from_value::<SandboxUnpublishedRecoveryProof>(encoded.clone()).unwrap(),
        old
    );
    assert_eq!(serde_json::to_value(&old).unwrap(), encoded);
    let requested = reserved_request();
    let after = apply_unpublished_runtime_recovery(&requested.snapshot, &requested)
        .unwrap()
        .sandbox;
    let encoded = serde_json::to_value(&after).unwrap();
    let decoded: Sandbox = serde_json::from_value(encoded).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, after);
}

#[test]
fn quiescence_and_next_reservation_replay_keep_original_reserved_evidence() {
    let requested = reserved_request();
    let mut after = apply_unpublished_runtime_recovery(&requested.snapshot, &requested)
        .unwrap()
        .sandbox;
    let proof = after
        .lifecycle
        .unpublished_recovery
        .as_ref()
        .unwrap()
        .proof
        .clone();
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
    assert_eq!(
        after.lifecycle.unpublished_recovery.as_ref().unwrap().proof,
        proof
    );
    let encoded = serde_json::to_vec(&after).unwrap();
    let decoded: Sandbox = serde_json::from_slice(&encoded).unwrap();
    decoded.validate().unwrap();
    assert_eq!(
        decoded
            .lifecycle
            .runtime_provision
            .as_ref()
            .unwrap()
            .generation,
        3
    );
    assert_eq!(
        decoded.retained_home_recovery().unwrap().source_generation,
        1
    );
}
