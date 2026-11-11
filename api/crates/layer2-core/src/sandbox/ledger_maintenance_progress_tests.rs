use super::*;
use crate::sandbox::ledger_maintenance::{
    check_ledger_writer, LedgerMaintenanceObservation, LedgerWriterClass,
};
use crate::sandbox::ledger_maintenance_admission::{
    plan_ledger_maintenance_admission, LedgerMaintenanceAdmissionRequest,
    LedgerMaintenanceAdmissionV1, LedgerMaintenanceLeaseV1,
};
use crate::sandbox::ledger_maintenance_plan::LedgerMaintenancePlanArtifactV1;
use crate::{Authorization, SubjectId};

const ENABLED: Policy = Policy { enabled: true };
const NOW: &str = "2026-09-05T00:30:02Z";
const LATER: &str = "2026-09-05T00:30:03Z";
const EXPIRED: &str = "2026-09-05T00:31:02Z";

fn request() -> LedgerMaintenanceProgress {
    let plan = crate::sandbox::ledger_maintenance_admission::test_support::fixture();
    let caller = Caller::new(
        plan.source.sandbox.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let accepted =
        LedgerMaintenancePlanArtifactV1::parse(&serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
    let admitted = plan_ledger_maintenance_admission(
        ENABLED,
        &plan.source,
        None,
        LedgerMaintenanceAdmissionRequest {
            caller: &caller,
            accepted: &accepted,
            lease: LedgerMaintenanceLeaseV1 {
                holder: "fixture-worker".into(),
                expires_at: EXPIRED.into(),
            },
            now: NOW,
        },
    )
    .unwrap();
    let LedgerMaintenanceAdmissionV1::Persist(state) = admitted else {
        panic!("initial admission")
    };
    LedgerMaintenanceProgress {
        caller,
        expected: *state,
        expected_source: plan.source,
        holder: "fixture-worker".into(),
        now: LATER.into(),
        action: LedgerMaintenanceEarlyAction::BeginDrain,
    }
}

fn apply(request: &LedgerMaintenanceProgress) -> Result<LedgerMaintenanceStateV1, Refusal> {
    plan_ledger_maintenance_progress(
        ENABLED,
        &request.expected,
        &request.expected_source,
        request,
    )
}

#[test]
fn draining_and_abort_keep_the_hold_and_original_plan_without_renewal() {
    let input = request();
    let before = serde_json::to_vec(&input.expected).unwrap();
    let draining = apply(&input).unwrap();
    assert_eq!(draining.record.phase, Phase::Draining);
    assert_eq!(
        draining.record.record_revision,
        input.expected.record.record_revision + 1
    );
    assert_eq!(draining.record.binding, input.expected.record.binding);
    assert!(draining.lease == input.expected.lease);
    assert_eq!(
        draining.accepted_plan_bytes,
        input.expected.accepted_plan_bytes
    );
    assert_eq!(serde_json::to_vec(&input.expected).unwrap(), before);
    let mut abort = input.clone();
    abort.expected = draining;
    abort.action = LedgerMaintenanceEarlyAction::AbortHeld;
    let held = apply(&abort).unwrap();
    assert_eq!(held.record.phase, Phase::AbortedHeld);
    assert_eq!(held.record.record_revision, 3);
    assert_eq!(held.record.binding, input.expected.record.binding);
    assert_eq!(held.accepted_plan_bytes, input.expected.accepted_plan_bytes);
    assert!(held.lease == input.expected.lease);
    assert!(check_ledger_writer(
        LedgerMaintenanceObservation::Present(&held.record),
        LedgerWriterClass::OwnerInput,
        None
    )
    .is_err());
    assert!(held.record.release_receipt_sha256.is_none());
    abort.expected = held;
    assert_eq!(apply(&abort), Err(Refusal::Conflict));
}

#[test]
fn default_off_and_foreign_callers_refuse_before_private_state_validation() {
    let mut input = request();
    assert_eq!(
        plan_ledger_maintenance_progress(
            Policy::default(),
            &input.expected,
            &input.expected_source,
            &input
        ),
        Err(Refusal::Disabled)
    );
    for caller in [
        Caller::new(
            SubjectId("foreign".into()),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
        Caller::new(input.caller.subject.clone(), Authorization::none()),
    ] {
        input.caller = caller;
        input.expected.accepted_plan_bytes = b"private malformed data".to_vec();
        assert_eq!(apply(&input), Err(Refusal::NotFound));
    }
    let mut input = request();
    input.caller = Caller::new(
        SubjectId("operator".into()),
        Authorization::new([Capability::ControlAnySandbox]),
    );
    assert_eq!(apply(&input).unwrap().record.phase, Phase::Draining);
}

#[test]
fn stale_holder_expired_lease_and_backwards_clock_cannot_advance() {
    for (holder, now, refusal) in [
        ("other-worker", LATER, Refusal::StaleLease),
        ("fixture-worker", EXPIRED, Refusal::StaleLease),
        (
            "fixture-worker",
            "2026-09-05T00:30:01Z",
            Refusal::InvalidEvidence,
        ),
        ("fixture-worker", "invalid", Refusal::InvalidEvidence),
    ] {
        let mut input = request();
        input.holder = holder.into();
        input.now = now.into();
        assert_eq!(apply(&input), Err(refusal));
    }
}

#[test]
fn every_authoritative_source_change_refuses_even_if_submitted_as_a_new_expected_snapshot() {
    for case in 0..10 {
        let mut input = request();
        match case {
            0 => input.expected_source.source_documents_sha256 = "b".repeat(64),
            1 => input.expected_source.sandbox.revision += 1,
            2 => input.expected_source.sandbox_event_head += 1,
            3 => input.expected_source.agents[0].revision += 1,
            4 => input.expected_source.agents[0].conversation.last_seq += 1,
            5 => input.expected_source.agents[0].work.next_fence += 1,
            6 => input.expected_source.workspaces[0].revision += 1,
            7 => input.expected_source.worktrees[0].revision += 1,
            8 => {
                input.expected_source.command_lanes[0]
                    .lane
                    .as_mut()
                    .unwrap()
                    .last_order += 1
            }
            _ => input.expected_source.command_lanes[0].lane = None,
        }
        assert_eq!(apply(&input), Err(Refusal::Conflict), "case {case}");
    }
}

#[test]
fn stale_record_and_any_later_phase_cannot_be_reopened_or_skip_evidence() {
    let input = request();
    let draining = apply(&input).unwrap();
    assert_eq!(
        plan_ledger_maintenance_progress(ENABLED, &draining, &input.expected_source, &input),
        Err(Refusal::Conflict)
    );
    for phase in [
        Phase::Excluded,
        Phase::BackedUp,
        Phase::Applying,
        Phase::HeldVerified,
        Phase::RestoreHeld,
        Phase::Released,
        Phase::AbortedHeld,
    ] {
        let mut input = request();
        input.expected.record.phase = phase;
        input.expected.record.authority_receipt_sha256 = Some("1".repeat(64));
        input.expected.record.backup_receipt_sha256 = Some("2".repeat(64));
        if phase == Phase::Released {
            input.expected.record.release_receipt_sha256 = Some("3".repeat(64));
        }
        input.action = LedgerMaintenanceEarlyAction::AbortHeld;
        assert_eq!(apply(&input), Err(Refusal::Conflict), "phase {phase:?}");
    }
    let mut input = request();
    input.expected.record.record_revision = u64::MAX;
    assert_eq!(apply(&input), Err(Refusal::InvalidEvidence));
}

#[test]
fn exact_original_request_reconstructs_the_same_outcome_after_serialized_restart() {
    let input = request();
    let expected = apply(&input).unwrap();
    let mut restarted = input.clone();
    restarted.expected =
        serde_json::from_slice(&serde_json::to_vec(&input.expected).unwrap()).unwrap();
    assert_eq!(apply(&restarted).unwrap(), expected);
    let mut changed = input.clone();
    changed.action = LedgerMaintenanceEarlyAction::AbortHeld;
    assert_ne!(apply(&changed).unwrap(), expected);
    assert_eq!(
        changed.expected.accepted_plan_bytes,
        input.expected.accepted_plan_bytes
    );
}
