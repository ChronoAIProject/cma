use super::*;
use journal::Stage;

const CUTS: [Stage; 4] = [
    Stage::Write,
    Stage::FileSync,
    Stage::Rename,
    Stage::DirectorySync,
];

#[tokio::test]
async fn interrupted_barrier_never_acks_and_restart_preserves_the_closed_gate() {
    for cut in CUTS {
        let mut f = Fixture::new().await;
        let dispatch = f.dispatch("first").await;
        let request = f.barrier(&dispatch);
        f.receiver
            .state
            .lock()
            .unwrap()
            .journal
            .as_mut()
            .unwrap()
            .fail_after = Some(cut);
        assert_eq!(
            f.receiver
                .barrier(f.owner.clone(), request.clone())
                .await
                .unwrap_err(),
            ProtocolError::Unavailable
        );
        assert!(!f.receiver.legacy_allowed());
        assert!(f.receiver.capabilities().is_none());
        assert!(f.receiver.state.lock().unwrap().gates[0].ready_to_observe_stop());
        f.restart(false);
        assert!(!f.receiver.legacy_allowed());
        assert!(f.receiver.capabilities().is_none());
        let stop = f
            .receiver
            .stop(f.owner.clone(), request, true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stop.evidence, StopEvidenceKind::RejectedBeforeStart);
        assert_eq!(f.runner.entries.load(Ordering::SeqCst), 0);
        assert!(f
            .receiver
            .start(f.authority.clone(), dispatch)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn interrupted_start_intent_never_replays_under_a_new_boot() {
    for cut in CUTS {
        let mut f = Fixture::new().await;
        let dispatch = f.dispatch("first").await;
        f.receiver
            .state
            .lock()
            .unwrap()
            .journal
            .as_mut()
            .unwrap()
            .fail_after = Some(cut);
        assert_eq!(
            f.receiver
                .start(f.authority.clone(), dispatch.clone())
                .await
                .unwrap_err(),
            ProtocolError::Unavailable
        );
        assert_eq!(f.runner.entries.load(Ordering::SeqCst), 0);
        f.restart(true);
        f.receiver.recover().await.unwrap();
        assert_eq!(
            f.receiver
                .start(f.authority.clone(), dispatch)
                .await
                .unwrap_err(),
            ProtocolError::AudienceMismatch
        );
        assert_eq!(f.runner.entries.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn interrupted_stop_retains_barrier_and_recovers_evidence_without_a_second_start() {
    for cut in CUTS {
        let mut f = Fixture::new().await;
        let dispatch = f.dispatch("first").await;
        let request = f.barrier(&dispatch);
        f.receiver
            .start(f.authority.clone(), dispatch)
            .await
            .unwrap();
        f.receiver
            .barrier(f.owner.clone(), request.clone())
            .await
            .unwrap();
        f.receiver
            .state
            .lock()
            .unwrap()
            .journal
            .as_mut()
            .unwrap()
            .fail_after = Some(cut);
        assert_eq!(
            f.receiver
                .stop(f.owner.clone(), request.clone(), true)
                .await
                .unwrap_err(),
            ProtocolError::Unavailable
        );
        f.restart(false);
        f.receiver.recover().await.unwrap();
        let stop = f
            .receiver
            .stop(f.owner.clone(), request.clone(), true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stop.evidence, StopEvidenceKind::CooperativeQuiescence);
        assert_eq!(stop.quiescence.effects.unresolved_count, 1);
        assert_eq!(
            f.receiver
                .stop(f.owner.clone(), request, false)
                .await
                .unwrap(),
            Some(stop)
        );
        assert_eq!(f.runner.entries.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn unknown_recovery_attempts_every_cell_and_keeps_admission_closed_but_control_available() {
    let mut f = Fixture::new().await;
    for name in ["first", "second"] {
        let dispatch = f.dispatch(name).await;
        f.receiver
            .start(f.authority.clone(), dispatch)
            .await
            .unwrap();
    }
    let request = {
        let state = f.receiver.state.lock().unwrap();
        let record = &state.snapshot.as_ref().unwrap().records[0];
        BarrierRequestV1 {
            protocol: 1,
            revocation: RevocationKeyV1 {
                operation_id: digest('5'),
                family: record
                    .ticket
                    .as_ref()
                    .unwrap()
                    .basis
                    .family()
                    .unwrap()
                    .clone(),
                watermark: generation(2),
            },
            member: MemberKey::Execution {
                execution: record.execution.clone(),
            },
            registration: digest('4'),
            reason: StopReason::Recovery,
        }
    };
    f.restart(true);
    f.runner.recovery_fails.store(true, Ordering::SeqCst);
    assert!(f.receiver.capabilities().is_none());
    assert_eq!(
        f.receiver.recover().await.unwrap_err(),
        ProtocolError::Unknown
    );
    assert_eq!(f.runner.recoveries.load(Ordering::SeqCst), 2);
    assert!(f.receiver.capabilities().is_none());
    f.receiver
        .barrier(f.owner.clone(), request.clone())
        .await
        .unwrap();
    f.runner.quiescent.store(false, Ordering::SeqCst);
    assert!(f
        .receiver
        .stop(f.owner.clone(), request.clone(), false)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        f.receiver
            .stop(f.owner.clone(), request, true)
            .await
            .unwrap_err(),
        ProtocolError::Unknown
    );
}

#[tokio::test]
async fn missing_marker_wrong_assignment_and_corrupt_state_refuse_startup() {
    for mutation in 0..3 {
        let f = Fixture::new().await;
        let root = f.root.join("receiver");
        let mut identity = f.receiver.identity.clone();
        match mutation {
            0 => std::fs::remove_file(root.with_extension("activated")).unwrap(),
            1 => identity.incarnation = digest('f'),
            _ => std::fs::write(root.join("state.json"), b"{}").unwrap(),
        }
        assert!(
            Receiver::with_runner(root, identity, false, f.runner.clone(), f.clock.clone())
                .is_err()
        );
        assert!(!f.receiver.legacy_allowed());
    }
}
