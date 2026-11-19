use super::*;
use crate::owner_protocol::{OwnershipFailureReason as Reason, OwnershipPhase as Phase};

fn owner() -> TransportOwner {
    TransportOwner {
        protocol: 1,
        agent_id: "agent".into(),
        instance_id: "manager".into(),
        epoch: 1,
    }
}
fn receiver() -> OwnershipReceiver {
    OwnershipReceiver {
        runtime_id: "runtime".into(),
        pod_uid: "pod".into(),
        container_id: "container".into(),
        restart_count: 0,
    }
}
fn failure() -> OwnershipFailure {
    OwnershipFailure {
        phase: Phase::Challenge,
        reason: Reason::ProtocolMismatch,
        http_status: Some(200),
        protocol: Some(2),
        lease_ms: Some(10000),
    }
}

#[test]
fn initial_rejection_and_renewal_rejection_have_distinct_confirmation_evidence() {
    for confirmed in [false, true] {
        let evidence = OwnershipEvidence::default();
        if confirmed {
            evidence.confirmation(Some(&receiver()), true);
        }
        evidence.record(owner(), Some(receiver()), failure(), true);
        let first = evidence.first_failure().unwrap();
        assert_eq!(first.owner(), &owner());
        assert_eq!(first.receiver(), Some(&receiver()));
        assert_eq!(first.failure(), failure());
        assert_eq!(first.confirmation_sent(), confirmed);
        assert_eq!(first.previously_confirmed(), confirmed);
        assert!(first.challenge_valid());
        evidence.record_unknown(owner(), OwnershipFailure::http(Phase::Revoke, 403));
        assert_eq!(evidence.first_failure().unwrap().failure(), failure());
    }
}

#[test]
fn uncertain_confirmation_and_distinct_receivers_cannot_invent_a_success() {
    let evidence = OwnershipEvidence::default();
    evidence.confirmation(Some(&receiver()), false);
    evidence.record(owner(), Some(receiver()), failure(), true);
    let first = evidence.first_failure().unwrap();
    assert!(first.confirmation_sent());
    assert!(!first.previously_confirmed());
    let evidence = OwnershipEvidence::default();
    evidence.confirmation(Some(&receiver()), true);
    let other = OwnershipReceiver {
        container_id: "other-container".into(),
        restart_count: 1,
        ..receiver()
    };
    evidence.record(owner(), Some(other.clone()), failure(), true);
    let first = evidence.first_failure().unwrap();
    assert_eq!(first.receiver(), Some(&other));
    assert!(!first.confirmation_sent());
    assert!(!first.previously_confirmed());
}

#[test]
fn uncorrelated_and_malformed_observations_retain_uncertainty() {
    for invalid in [
        OwnershipReceiver {
            runtime_id: String::new(),
            ..receiver()
        },
        OwnershipReceiver {
            pod_uid: "x".repeat(513),
            ..receiver()
        },
        OwnershipReceiver {
            container_id: String::new(),
            ..receiver()
        },
        OwnershipReceiver {
            restart_count: -1,
            ..receiver()
        },
    ] {
        let evidence = OwnershipEvidence::default();
        evidence.confirmation(Some(&invalid), false);
        evidence.record(owner(), Some(invalid), failure(), false);
        let first = evidence.first_failure().unwrap();
        assert!(first.receiver().is_none());
        assert!(first.confirmation_sent());
        assert!(!first.challenge_valid());
    }
    let evidence = OwnershipEvidence::default();
    evidence.record_unknown(owner(), OwnershipFailure::http(Phase::Challenge, 404));
    assert!(evidence.first_failure().unwrap().receiver().is_none());
}

#[test]
fn overlapping_negotiations_exclude_a_snapshot_before_the_late_confirmation() {
    let evidence = OwnershipEvidence::default();
    let first = evidence.negotiation();
    let second = evidence.negotiation();
    evidence.record(owner(), Some(receiver()), failure(), true);
    let snapshot = evidence.first_failure().unwrap();
    assert!(snapshot.confirmation_sent());
    assert!(snapshot.previously_confirmed());
    drop(first);
    evidence.confirmation(Some(&receiver()), true);
    drop(second);
    assert_eq!(snapshot.failure(), failure());
    assert!(evidence.first_failure().unwrap().confirmation_sent());
}

#[test]
fn completed_negotiations_do_not_invent_overlap() {
    let evidence = OwnershipEvidence::default();
    drop(evidence.negotiation());
    let _current = evidence.negotiation();
    evidence.record(owner(), Some(receiver()), failure(), true);
    assert!(!evidence.first_failure().unwrap().confirmation_sent());
}

#[test]
fn native_receiver_requires_complete_bounded_incarnation() {
    let binding = crate::agent_sandbox::PodBinding {
        sandbox_name: "sandbox".into(),
        sandbox_uid: "sandbox-uid".into(),
        pod_name: "pod".into(),
        pod_uid: "pod-uid".into(),
        pod_ip: "10.0.0.1".parse().unwrap(),
        runtime_container_id: Some("container".into()),
        runtime_restart_count: Some(0),
    };
    let handle = crate::RuntimeHandle::new("runtime");
    assert!(OwnershipReceiver::from_native(&handle, &binding).is_some());
    let mut missing_container = binding.clone();
    missing_container.runtime_container_id = None;
    let mut missing_restarts = binding.clone();
    missing_restarts.runtime_restart_count = None;
    let mut negative_restarts = binding.clone();
    negative_restarts.runtime_restart_count = Some(-1);
    let mut oversized = binding.clone();
    oversized.pod_uid = "x".repeat(513);
    for incomplete in [
        missing_container,
        missing_restarts,
        negative_restarts,
        oversized,
    ] {
        assert!(OwnershipReceiver::from_native(&handle, &incomplete).is_none());
    }
    assert!(OwnershipReceiver::from_native(&crate::RuntimeHandle::new(""), &binding).is_none());
}

#[test]
fn concurrent_confirmation_after_first_failure_prevents_initial_rejection_evidence() {
    for target in [Some(receiver()), None] {
        let evidence = OwnershipEvidence::default();
        evidence.record(owner(), Some(receiver()), failure(), true);
        assert!(!evidence.first_failure().unwrap().confirmation_sent());
        evidence.confirmation(target.as_ref(), true);
        let first = evidence.first_failure().unwrap();
        assert_eq!(first.failure(), failure());
        assert_eq!(first.receiver(), Some(&receiver()));
        assert!(first.confirmation_sent());
        assert!(first.previously_confirmed());
    }
}
