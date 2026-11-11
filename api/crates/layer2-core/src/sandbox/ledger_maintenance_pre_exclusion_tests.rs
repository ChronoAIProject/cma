use super::*;
use crate::sandbox::ledger_maintenance::{LedgerMaintenanceRefusal, LedgerWriterClass};
use crate::sandbox::ledger_maintenance_admission::test_support;
use crate::sandbox::ledger_maintenance_admission::LedgerMaintenanceStateV1;
use crate::sandbox::ledger_maintenance_plan::LedgerMaintenancePlanArtifactV1;
use crate::sandbox::ledger_maintenance_pre_exclusion_progress::*;
use crate::{Authorization, Caller, Capability};

const NOW: &str = "2026-09-05T00:30:02Z";
const EXPIRED: &str = "2026-09-05T00:31:02Z";
const NEXT: &str = "2026-09-05T00:32:02Z";

fn fixture() -> LedgerPreExclusionPlanV1 {
    let prior = test_support::fixture();
    LedgerPreExclusionPlanV1 {
        format: "ledger-pre-exclusion-plan-v1".into(),
        operation_id: "pre-exclusion-fixture-operation".into(),
        sandbox_id: prior.source.sandbox.id.clone(),
        home_identity: prior.binding.home_identity,
        generation: prior.binding.generation,
        boot_id: prior.binding.boot_id,
        roots_sha256: prior.binding.roots_sha256,
        authorized_by: prior.source.sandbox.owner.clone(),
        source: prior.source,
        disposition: LedgerRecoveryDispositionV1::PreserveHeld,
        consent_receipt_sha256: prior.consent_receipt_sha256,
        writer_barrier_epoch: 1,
        compatible_floor: LedgerMaintenanceFloorV1 {
            reader_manifest_sha256: "a".repeat(64),
            writer_manifest_sha256: "b".repeat(64),
        },
        requested_at: NOW.into(),
    }
}

fn request() -> (LedgerPreExclusionPolicy, LedgerPreExclusionAdmission) {
    let plan = fixture();
    let policy = LedgerPreExclusionPolicy {
        enabled: true,
        qualified_floor: Some(plan.compatible_floor.clone()),
    };
    let input = LedgerPreExclusionAdmission {
        caller: Caller::new(
            plan.authorized_by.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        accepted: LedgerPreExclusionArtifactV1::parse(&serde_json::to_vec_pretty(&plan).unwrap())
            .unwrap(),
        lease: LedgerMaintenanceLeaseV1 {
            holder: "pre-exclusion-fixture-worker".into(),
            expires_at: EXPIRED.into(),
        },
        now: NOW.into(),
    };
    (policy, input)
}

fn admitted() -> (LedgerPreExclusionPolicy, LedgerPreExclusionProgress) {
    let (policy, input) = request();
    let source = input.accepted.plan().source.clone();
    let LedgerPreExclusionAdmissionDecision::Persist(state) =
        plan_pre_exclusion_admission(&policy, &source, None, &input).unwrap()
    else {
        panic!("first admission must persist")
    };
    (
        policy,
        LedgerPreExclusionProgress {
            caller: input.caller,
            expected: *state,
            expected_source: source,
            holder: input.lease.holder,
            now: NOW.into(),
            action: LedgerPreExclusionAction::BeginDrain,
        },
    )
}

#[test]
fn pre_exclusion_artifact_is_distinct_and_never_manufactures_execution_evidence() {
    let (_, input) = request();
    assert!(LedgerMaintenancePlanArtifactV1::parse(input.accepted.original_bytes()).is_err());
    let original = serde_json::to_vec_pretty(&test_support::fixture()).unwrap();
    let v1 = LedgerMaintenancePlanArtifactV1::parse(&original).unwrap();
    assert_eq!(v1.original_bytes(), original);
    assert!(LedgerPreExclusionArtifactV1::parse(&original).is_err());
    let value: serde_json::Value = serde_json::from_slice(input.accepted.original_bytes()).unwrap();
    for field in [
        "binding",
        "exclusion_receipt_sha256",
        "archive_id",
        "backup_receipt_sha256",
    ] {
        assert!(value.get(field).is_none());
        let mut bad = value.clone();
        bad[field] = serde_json::json!("a".repeat(64));
        assert!(LedgerPreExclusionArtifactV1::parse(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
}

#[test]
fn identities_source_floor_and_owner_disposition_are_bound_and_validated() {
    for case in 0..11 {
        let mut bad = fixture();
        match case {
            0 => bad.home_identity.push('x'),
            1 => bad.generation += 1,
            2 => bad.boot_id.push('x'),
            3 => bad.sandbox_id.0.push('x'),
            4 => bad.authorized_by.0.push('x'),
            5 => bad.operation_id.clear(),
            6 => bad.writer_barrier_epoch = 2,
            7 => bad.compatible_floor.reader_manifest_sha256.clear(),
            8 => bad.consent_receipt_sha256.clear(),
            9 => bad.source.command_lanes.clear(),
            _ => bad.source.source_documents_sha256.clear(),
        }
        assert_eq!(bad.validate(), Err(Refusal::InvalidEvidence), "case {case}");
    }
}

#[test]
fn disabled_unqualified_floor_and_unauthorized_callers_cannot_admit() {
    let (policy, input) = request();
    let source = &input.accepted.plan().source;
    for bad_policy in [
        LedgerPreExclusionPolicy::default(),
        LedgerPreExclusionPolicy {
            enabled: true,
            qualified_floor: None,
        },
        LedgerPreExclusionPolicy {
            enabled: true,
            qualified_floor: Some(LedgerMaintenanceFloorV1 {
                reader_manifest_sha256: "c".repeat(64),
                writer_manifest_sha256: "b".repeat(64),
            }),
        },
    ] {
        assert!(plan_pre_exclusion_admission(&bad_policy, source, None, &input).is_err());
    }
    for caller in [
        Caller::new(source.sandbox.owner.clone(), Authorization::none()),
        Caller::new(
            SubjectId("foreign-fixture".into()),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
    ] {
        let mut bad = input.clone();
        bad.caller = caller;
        assert_eq!(
            plan_pre_exclusion_admission(&policy, source, None, &bad),
            Err(Refusal::NotFound)
        );
    }
}

#[test]
fn restart_and_expired_lease_replay_exact_bytes_without_renewal() {
    let (policy, progress) = admitted();
    let original = progress.expected;
    let restarted: LedgerPreExclusionStateV1 =
        serde_json::from_slice(&serde_json::to_vec(&original).unwrap()).unwrap();
    let (_, mut retry) = request();
    retry.now = EXPIRED.into();
    retry.lease.expires_at = NEXT.into();
    let outcome =
        plan_pre_exclusion_admission(&policy, &progress.expected_source, Some(&restarted), &retry)
            .unwrap();
    assert_eq!(
        outcome,
        LedgerPreExclusionAdmissionDecision::Replay(Box::new(original.clone()))
    );
    assert_eq!(
        original.accepted_admission_bytes,
        retry.accepted.original_bytes()
    );
    assert!(serde_json::from_slice::<LedgerMaintenanceStateV1>(
        &serde_json::to_vec(&original).unwrap()
    )
    .is_err());
}

#[test]
fn semantically_equal_reserialization_and_replacement_operations_conflict() {
    let (policy, progress) = admitted();
    for change_operation in [false, true] {
        let (_, mut retry) = request();
        let mut plan = retry.accepted.plan().clone();
        if change_operation {
            plan.operation_id.push('x');
        }
        retry.accepted =
            LedgerPreExclusionArtifactV1::parse(&serde_json::to_vec(&plan).unwrap()).unwrap();
        assert_eq!(
            plan_pre_exclusion_admission(
                &policy,
                &progress.expected_source,
                Some(&progress.expected),
                &retry
            ),
            Err(Refusal::Conflict)
        );
    }
}

#[test]
fn draining_and_abort_retain_original_source_bytes_epoch_and_lease() {
    let (policy, mut input) = admitted();
    let original = input.expected.clone();
    let draining =
        plan_pre_exclusion_progress(&policy, &original, &input.expected_source, &input).unwrap();
    assert_eq!(draining.phase, LedgerPreExclusionPhaseV1::Draining);
    assert_eq!(draining.record_revision, 2);
    input.expected = draining;
    input.action = LedgerPreExclusionAction::AbortHeld;
    let aborted =
        plan_pre_exclusion_progress(&policy, &input.expected, &input.expected_source, &input)
            .unwrap();
    assert_eq!(aborted.phase, LedgerPreExclusionPhaseV1::AbortedHeld);
    assert_eq!(aborted.record_revision, 3);
    assert_eq!(
        aborted.accepted_admission_bytes,
        original.accepted_admission_bytes
    );
    assert_eq!(aborted.writer_barrier_epoch, original.writer_barrier_epoch);
    assert!(aborted.lease == original.lease);
    input.expected = aborted;
    assert_eq!(
        plan_pre_exclusion_progress(&policy, &input.expected, &input.expected_source, &input),
        Err(Refusal::Conflict)
    );
}

#[test]
fn takeover_increments_epoch_without_inheriting_a_permit_and_losing_epoch_refuses() {
    let (policy, mut input) = admitted();
    let original = input.expected.clone();
    input.action = LedgerPreExclusionAction::Takeover {
        lease: LedgerMaintenanceLeaseV1 {
            holder: "replacement-fixture-worker".into(),
            expires_at: NEXT.into(),
        },
    };
    input.holder = "replacement-fixture-worker".into();
    assert_eq!(
        plan_pre_exclusion_progress(&policy, &original, &input.expected_source, &input),
        Err(Refusal::StaleLease)
    );
    input.now = EXPIRED.into();
    let next =
        plan_pre_exclusion_progress(&policy, &original, &input.expected_source, &input).unwrap();
    assert_eq!(next.writer_barrier_epoch, 2);
    assert_eq!(next.record_revision, 2);
    assert_eq!(next.phase, LedgerPreExclusionPhaseV1::Requested);
    assert_eq!(
        next.accepted_admission_bytes,
        original.accepted_admission_bytes
    );
    assert_eq!(
        plan_pre_exclusion_progress(&policy, &next, &input.expected_source, &input),
        Err(Refusal::Conflict)
    );
}

#[test]
fn refreshed_source_cannot_silently_replace_the_accepted_snapshot() {
    let (policy, mut input) = admitted();
    input.expected_source.source_documents_sha256 = "d".repeat(64);
    assert_eq!(
        plan_pre_exclusion_progress(&policy, &input.expected, &input.expected_source, &input),
        Err(Refusal::Conflict)
    );
}

#[test]
fn stale_expired_and_backward_time_progress_is_refused() {
    for case in 0..4 {
        let (policy, mut input) = admitted();
        match case {
            0 => input.holder.push('x'),
            1 => input.now = EXPIRED.into(),
            2 => input.now = "2026-09-05T00:30:01Z".into(),
            _ => input.expected.record_revision = u64::MAX,
        }
        assert!(
            plan_pre_exclusion_progress(&policy, &input.expected, &input.expected_source, &input)
                .is_err(),
            "case {case}"
        );
    }
}

#[test]
fn every_writer_and_purge_is_held_in_every_phase_with_no_expiry_release() {
    use LedgerWriterClass::*;
    let (_, input) = admitted();
    for phase in [
        LedgerPreExclusionPhaseV1::Requested,
        LedgerPreExclusionPhaseV1::Draining,
        LedgerPreExclusionPhaseV1::AbortedHeld,
    ] {
        let mut state = input.expected.clone();
        state.phase = phase;
        for class in [
            OwnerInput,
            OwnerControl,
            Creation,
            AgentWorker,
            WorkspaceWorker,
            Recorder,
            Checkpoint,
            Output,
            Reconciliation,
            Recovery,
            Rotation,
            Revival,
            Credentials,
            Retirement,
            HomeCleanup,
            Purge,
            Administration,
            Provider,
        ] {
            assert_eq!(
                state.check_writer(class),
                Err(LedgerMaintenanceRefusal::Held)
            );
        }
    }
}

#[test]
fn unknown_states_and_tampered_bytes_fail_closed_and_debug_has_no_identifiers() {
    let (_, input) = admitted();
    let state = input.expected;
    let mut json = serde_json::to_value(&state).unwrap();
    json["phase"] = serde_json::json!("released");
    assert!(serde_json::from_value::<LedgerPreExclusionStateV1>(json).is_err());
    let mut corrupt = state.clone();
    corrupt.accepted_admission_bytes.push(b' ');
    assert_eq!(
        corrupt.check_writer(LedgerWriterClass::Purge),
        Err(LedgerMaintenanceRefusal::InvalidEvidence)
    );
    let debug = format!("{state:?} {:?}", state.accepted().unwrap());
    for private in [
        "pre-exclusion-fixture-operation",
        "pre-exclusion-fixture-worker",
        "sandbox_id",
        "requested_by",
        "home_identity",
    ] {
        assert!(!debug.contains(private));
    }
}
