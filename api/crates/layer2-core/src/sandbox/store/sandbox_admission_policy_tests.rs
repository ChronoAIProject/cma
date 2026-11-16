use super::*;
use crate::sandbox::lifecycle_test_support::{sandbox, NOW};

fn admission(always_on: Option<bool>) -> SandboxAdmission {
    let mut value = sandbox();
    value.origin = SandboxOrigin::Lifecycle {
        operation_id: "policy-proof".into(),
    };
    value.status = SandboxStatus::Provisioning;
    value.runtime = None;
    value.revision = 1;
    value.profile.always_on = always_on;
    value.profile.digest = content_digest(&value.profile.content).unwrap();
    SandboxAdmission::new(
        value.clone(),
        SandboxAdmissionName::Exact(value.name),
        3,
        NOW.into(),
    )
    .unwrap()
}

#[test]
fn retained_policy_proof_preserves_all_original_and_current_states() {
    for original_policy in [None, Some(false), Some(true)] {
        let original = admission(original_policy);
        for current_policy in [None, Some(false), Some(true)] {
            let mut current = original.sandbox.clone();
            current.profile.always_on = current_policy;
            current.revision += 1;
            let replay = SandboxAdmissionReplay::from_retained(
                current.clone(),
                original.name.clone(),
                original.admission_fingerprint.clone(),
            )
            .unwrap();
            assert_eq!(replay.admitted_always_on, original_policy);
            assert_eq!(replay.sandbox, current);
            assert_eq!(replay.admission_fingerprint, original.admission_fingerprint);
            replay.validate().unwrap();
            replay.clone().validate().unwrap();
        }
    }
}

#[test]
fn fresh_admission_and_fresh_replay_require_the_exact_initial_policy() {
    for original_policy in [None, Some(false), Some(true)] {
        let original = admission(original_policy);
        for supplied_policy in [None, Some(false), Some(true)] {
            let mut changed = original.clone();
            changed.sandbox.profile.always_on = supplied_policy;
            assert_eq!(
                changed.validate().is_ok(),
                original_policy == supplied_policy
            );
            assert_eq!(
                SandboxAdmissionReplay::new(
                    changed.sandbox,
                    original.name.clone(),
                    original.admission_fingerprint.clone(),
                )
                .is_ok(),
                original_policy == supplied_policy,
            );
            let new = admission(supplied_policy);
            assert_eq!(
                original.admission_fingerprint == new.admission_fingerprint,
                original_policy == supplied_policy,
            );
        }
    }
}

#[test]
fn retained_proof_remains_bound_during_later_service_revalidation() {
    let original = admission(None);
    let mut current = original.sandbox.clone();
    current.profile.always_on = Some(true);
    let replay = SandboxAdmissionReplay::from_retained(
        current,
        original.name,
        original.admission_fingerprint,
    )
    .unwrap();
    let mut new_policy = replay.clone();
    new_policy.sandbox.profile.always_on = Some(false);
    new_policy.validate().unwrap();
    for mutation in 0..5 {
        let mut changed = replay.clone();
        match mutation {
            0 => changed.admitted_always_on = Some(true),
            1 => changed.sandbox.owner = SubjectId::new("another-owner"),
            2 => changed.sandbox.profile.revision += 1,
            3 => changed.admission_fingerprint = "0".repeat(64),
            _ => changed.name = SandboxAdmissionName::Exact("another-name".into()),
        }
        assert!(changed.validate().is_err(), "mutation {mutation}");
    }
}

#[test]
fn retained_policy_matching_does_not_hide_invalid_current_state_or_digest() {
    let original = admission(None);
    for mutation in 0..6 {
        let mut changed = original.sandbox.clone();
        changed.profile.always_on = Some(true);
        match mutation {
            0 => changed.status = SandboxStatus::Failed,
            1 => changed.revision = 0,
            2 => changed.profile.digest = "0".repeat(64),
            3 => changed.profile.content.cpu_millis += 1,
            4 => changed.origin = SandboxOrigin::Manual,
            _ => changed.limits.max_workspaces = 0,
        }
        assert!(
            SandboxAdmissionReplay::from_retained(
                changed,
                original.name.clone(),
                original.admission_fingerprint.clone(),
            )
            .is_err(),
            "mutation {mutation}",
        );
    }
}
