use super::super::ledger_maintenance_plan::*;
use super::*;
use crate::Authorization;

use super::test_support::fixture;

const NOW: &str = "2026-09-05T00:30:02Z";
const EXPIRED: &str = "2026-09-05T00:31:02Z";
const NEXT: &str = "2026-09-05T00:32:02Z";

fn owner() -> Caller {
    Caller::new(
        SubjectId("owner".into()),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

fn artifact(plan: &LedgerMaintenancePlanV1) -> LedgerMaintenancePlanArtifactV1 {
    LedgerMaintenancePlanArtifactV1::parse(&serde_json::to_vec(plan).unwrap()).unwrap()
}
fn lease(at: &str) -> LedgerMaintenanceLeaseV1 {
    LedgerMaintenanceLeaseV1 {
        holder: "fixture-worker".into(),
        expires_at: at.into(),
    }
}
fn admit(plan: &LedgerMaintenancePlanArtifactV1) -> LedgerMaintenanceStateV1 {
    let result = plan_ledger_maintenance_admission(
        LedgerMaintenanceAdmissionPolicy { enabled: true },
        &plan.plan().source,
        None,
        LedgerMaintenanceAdmissionRequest {
            caller: &owner(),
            accepted: plan,
            lease: lease(EXPIRED),
            now: NOW,
        },
    )
    .unwrap();
    let LedgerMaintenanceAdmissionV1::Persist(next) = result else {
        panic!("first admission must persist")
    };
    *next
}

#[test]
fn admission_defaults_off_and_authorization_is_not_derived_from_plan_identity() {
    let plan = artifact(&fixture());
    assert_eq!(
        plan_ledger_maintenance_admission(
            LedgerMaintenanceAdmissionPolicy::default(),
            &plan.plan().source,
            None,
            LedgerMaintenanceAdmissionRequest {
                caller: &owner(),
                accepted: &plan,
                lease: lease(EXPIRED),
                now: NOW
            },
        ),
        Err(LedgerMaintenanceAdmissionRefusal::Disabled)
    );
    for caller in [
        Caller::new(SubjectId("owner".into()), Authorization::none()),
        Caller::new(
            SubjectId("foreign".into()),
            Authorization::new([Capability::UseOwnSandboxes]),
        ),
    ] {
        assert_eq!(
            plan_ledger_maintenance_admission(
                LedgerMaintenanceAdmissionPolicy { enabled: true },
                &plan.plan().source,
                None,
                LedgerMaintenanceAdmissionRequest {
                    caller: &caller,
                    accepted: &plan,
                    lease: lease(EXPIRED),
                    now: NOW
                },
            ),
            Err(LedgerMaintenanceAdmissionRefusal::NotFound)
        );
    }
}

#[test]
fn exact_retry_survives_restart_and_does_not_renew_lease_or_replace_original_bytes() {
    let plan = artifact(&fixture());
    let admitted = admit(&plan);
    let restarted: LedgerMaintenanceStateV1 =
        serde_json::from_slice(&serde_json::to_vec(&admitted).unwrap()).unwrap();
    let result = plan_ledger_maintenance_admission(
        LedgerMaintenanceAdmissionPolicy { enabled: true },
        &plan.plan().source,
        Some(&restarted),
        LedgerMaintenanceAdmissionRequest {
            caller: &owner(),
            accepted: &plan,
            lease: lease(NEXT),
            now: EXPIRED,
        },
    )
    .unwrap();
    assert_eq!(
        result,
        LedgerMaintenanceAdmissionV1::Replay(Box::new(admitted.clone()))
    );
    assert_eq!(admitted.accepted_plan_bytes, plan.original_bytes());
    assert_eq!(admitted.record.phase, LedgerMaintenancePhaseV1::Requested);
    let reordered =
        LedgerMaintenancePlanArtifactV1::parse(&serde_json::to_vec_pretty(plan.plan()).unwrap())
            .unwrap();
    assert_eq!(
        plan_ledger_maintenance_admission(
            LedgerMaintenanceAdmissionPolicy { enabled: true },
            &plan.plan().source,
            Some(&restarted),
            LedgerMaintenanceAdmissionRequest {
                caller: &owner(),
                accepted: &reordered,
                lease: lease(NEXT),
                now: EXPIRED
            },
        ),
        Err(LedgerMaintenanceAdmissionRefusal::Conflict)
    );
}

#[test]
fn admission_compares_each_resource_log_and_command_lane_component() {
    let plan = artifact(&fixture());
    for case in 0..9 {
        let mut changed = plan.plan().source.clone();
        match case {
            0 => changed.sandbox.revision += 1,
            1 => changed.sandbox_event_head += 1,
            2 => changed.agents[0].revision += 1,
            3 => changed.agents[0].conversation.last_seq += 1,
            4 => changed.agents[0].work.next_fence += 1,
            5 => changed.workspaces[0].revision += 1,
            6 => changed.worktrees[0].revision += 1,
            7 => changed.command_lanes[0].lane.as_mut().unwrap().last_order += 1,
            _ => changed.command_lanes[0].lane.as_mut().unwrap().next_fence += 1,
        }
        assert_eq!(
            plan_ledger_maintenance_admission(
                LedgerMaintenanceAdmissionPolicy { enabled: true },
                &changed,
                None,
                LedgerMaintenanceAdmissionRequest {
                    caller: &owner(),
                    accepted: &plan,
                    lease: lease(EXPIRED),
                    now: NOW
                },
            ),
            Err(LedgerMaintenanceAdmissionRefusal::Conflict)
        );
    }
}

#[test]
fn one_home_cannot_admit_another_operation_while_the_first_is_retained() {
    let plan = artifact(&fixture());
    let current = admit(&plan);
    let mut other = fixture();
    other.binding.maintenance_id = "other-operation".into();
    let other = artifact(&other);
    assert_eq!(
        plan_ledger_maintenance_admission(
            LedgerMaintenanceAdmissionPolicy { enabled: true },
            &plan.plan().source,
            Some(&current),
            LedgerMaintenanceAdmissionRequest {
                caller: &owner(),
                accepted: &other,
                lease: lease(EXPIRED),
                now: NOW
            },
        ),
        Err(LedgerMaintenanceAdmissionRefusal::Conflict)
    );
}

#[test]
fn expired_takeover_advances_only_authority_and_stale_worker_cannot_retake_it() {
    let plan = artifact(&fixture());
    let current = admit(&plan);
    let next = plan_ledger_maintenance_takeover(
        &current,
        &plan.plan().source,
        LedgerMaintenanceTakeoverRequest {
            caller: &owner(),
            expected: &current,
            expected_source: &plan.plan().source,
            lease: lease(NEXT),
            now: EXPIRED,
        },
    )
    .unwrap();
    assert_eq!(next.accepted_plan_bytes, current.accepted_plan_bytes);
    assert_eq!(next.record.plan_sha256, current.record.plan_sha256);
    assert_eq!(
        next.record.binding.maintenance_id,
        current.record.binding.maintenance_id
    );
    assert_eq!(next.record.binding.maintenance_epoch, 2);
    assert_eq!(next.record.record_revision, 2);
    assert_eq!(next.record.phase, LedgerMaintenancePhaseV1::Requested);
    assert_eq!(
        plan_ledger_maintenance_takeover(
            &next,
            &plan.plan().source,
            LedgerMaintenanceTakeoverRequest {
                caller: &owner(),
                expected: &current,
                expected_source: &plan.plan().source,
                lease: lease(NEXT),
                now: EXPIRED
            },
        ),
        Err(LedgerMaintenanceAdmissionRefusal::Conflict)
    );
    assert_eq!(
        plan_ledger_maintenance_takeover(
            &current,
            &plan.plan().source,
            LedgerMaintenanceTakeoverRequest {
                caller: &owner(),
                expected: &current,
                expected_source: &plan.plan().source,
                lease: lease(NEXT),
                now: NOW
            },
        ),
        Err(LedgerMaintenanceAdmissionRefusal::StaleLease)
    );
}

#[test]
fn takeover_refuses_new_source_effects_even_with_a_freshly_supplied_expected_snapshot() {
    let plan = artifact(&fixture());
    let current = admit(&plan);
    let mut changed = plan.plan().source.clone();
    changed.agents[0].conversation.last_seq += 1;
    assert_eq!(
        plan_ledger_maintenance_takeover(
            &current,
            &changed,
            LedgerMaintenanceTakeoverRequest {
                caller: &owner(),
                expected: &current,
                expected_source: &changed,
                lease: lease(NEXT),
                now: EXPIRED
            },
        ),
        Err(LedgerMaintenanceAdmissionRefusal::Conflict)
    );
}

#[test]
fn invalid_foreign_missing_lane_and_changed_home_plans_refuse_before_admission() {
    for case in 0..6 {
        let mut plan = fixture();
        match case {
            0 => plan.binding.home_identity = "x".repeat(43),
            1 => plan.binding.boot_id = "c".repeat(32),
            2 => plan.source.command_lanes.clear(),
            3 => plan.source.agents[0].owner = SubjectId("foreign".into()),
            4 => plan
                .source
                .command_lanes
                .push(plan.source.command_lanes[0].clone()),
            _ => plan.consent_receipt_sha256.clear(),
        }
        assert!(
            LedgerMaintenancePlanArtifactV1::parse(&serde_json::to_vec(&plan).unwrap()).is_err()
        );
    }
    assert!(matches!(
        LedgerMaintenancePlanArtifactV1::parse(&vec![b' '; MAX_LEDGER_MAINTENANCE_PLAN_BYTES + 1]),
        Err(LedgerMaintenancePlanRefusal::Oversized)
    ));
}
