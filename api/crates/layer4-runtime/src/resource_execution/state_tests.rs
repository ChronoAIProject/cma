use super::*;

#[tokio::test]
async fn barrier_before_start_returns_real_no_entry_receipt_and_is_replayable() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let request = f.barrier(&dispatch);
    f.receiver
        .barrier(f.owner.clone(), request.clone())
        .await
        .unwrap();
    assert_eq!(
        f.receiver
            .start(f.authority.clone(), dispatch)
            .await
            .unwrap_err(),
        ProtocolError::Expired
    );
    let receipt = f
        .receiver
        .stop(f.owner.clone(), request.clone(), true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.evidence, StopEvidenceKind::RejectedBeforeStart);
    assert_eq!(receipt.quiescence.effects.high_water, 0);
    assert_eq!(f.runner.entries.load(Ordering::SeqCst), 0);
    assert_eq!(
        Some(receipt),
        f.receiver
            .stop(f.owner.clone(), request, false)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn barrier_during_pending_prepare_prevents_enter_and_waits_for_prepare_owner() {
    let f = Fixture::new().await;
    f.runner.hold_prepare.store(true, Ordering::SeqCst);
    let dispatch = f.dispatch("first").await;
    let request = f.barrier(&dispatch);
    let receiver = f.receiver.clone();
    let authority = f.authority.clone();
    let start = tokio::spawn(async move { receiver.start(authority, dispatch).await });
    f.runner.preparing.notified().await;
    f.receiver
        .barrier(f.owner.clone(), request.clone())
        .await
        .unwrap();
    assert_eq!(
        f.receiver
            .stop(f.owner.clone(), request.clone(), true)
            .await
            .unwrap_err(),
        ProtocolError::Unknown
    );
    f.runner.release.notify_one();
    assert_eq!(start.await.unwrap().unwrap_err(), ProtocolError::Expired);
    let receipt = f
        .receiver
        .stop(f.owner.clone(), request, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.evidence, StopEvidenceKind::RejectedBeforeStart);
    assert_eq!(f.runner.entries.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn duplicate_start_recovers_evidence_without_executing_again() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let first = f
        .receiver
        .start(f.authority.clone(), dispatch.clone())
        .await
        .unwrap();
    f.clock.set(5000);
    let replay = f
        .receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    assert_eq!(first, replay);
    assert_eq!(f.runner.entries.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stop_needs_observed_quiescence_and_preserves_unknown_remote_effects() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let request = f.barrier(&dispatch);
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    f.runner.quiescent.store(false, Ordering::SeqCst);
    assert_eq!(
        f.receiver
            .stop(f.owner.clone(), request.clone(), true)
            .await
            .unwrap_err(),
        ProtocolError::Unknown
    );
    assert!(f
        .receiver
        .stop(f.owner.clone(), request.clone(), false)
        .await
        .unwrap()
        .is_none());
    f.runner.quiescent.store(true, Ordering::SeqCst);
    let receipt = f
        .receiver
        .stop(f.owner.clone(), request, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.evidence, StopEvidenceKind::CooperativeQuiescence);
    assert_eq!(receipt.quiescence.effects.unresolved_count, 1);
}

#[tokio::test]
async fn delayed_continuation_does_not_extend_its_original_challenge() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let ticket = dispatch.ticket().clone();
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    f.clock.set(1100);
    let challenge = f
        .receiver
        .challenge(f.owner.clone(), ticket.execution.clone())
        .await
        .unwrap();
    f.clock.set(3000);
    let lease = ContinuationV1 {
        protocol: 1,
        execution: ticket.execution,
        registration: ticket.registration,
        basis: ticket.basis,
        manager: ticket.manager,
        challenge,
        sequence: generation(2),
    };
    f.receiver
        .continue_execution(f.authority.clone(), lease.clone())
        .await
        .unwrap();
    f.clock.set(4100);
    assert_eq!(
        f.receiver
            .continue_execution(f.authority.clone(), lease)
            .await
            .unwrap_err(),
        ProtocolError::Expired
    );
}

#[tokio::test]
async fn queued_start_cannot_renew_and_wrong_successor_cannot_refresh_running_work() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let ticket = dispatch.ticket().clone();
    f.clock.set(1100);
    assert_eq!(
        f.receiver
            .challenge(f.owner.clone(), ticket.execution.clone())
            .await
            .unwrap(),
        ticket.challenge
    );
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    let challenge = f
        .receiver
        .challenge(f.owner.clone(), ticket.execution.clone())
        .await
        .unwrap();
    let lease = ContinuationV1 {
        protocol: 1,
        execution: ticket.execution,
        registration: ticket.registration,
        basis: ticket.basis,
        manager: ticket.manager,
        challenge,
        sequence: generation(3),
    };
    assert_eq!(
        f.receiver
            .continue_execution(f.authority.clone(), lease)
            .await
            .unwrap_err(),
        ProtocolError::EvidenceMismatch
    );
}

#[tokio::test]
async fn watchdog_closes_and_escalates_while_the_journal_lock_is_stalled() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    let gate = f.receiver.state.lock().unwrap().gates[0].clone();
    let _journal = f.receiver.state.lock().unwrap();
    f.clock.set(3100);
    wait_thread(|| gate.ready_to_observe_stop());
    f.clock.set(5100);
    wait_thread(|| f.runner.forced.load(Ordering::SeqCst) > 0);
}

#[tokio::test]
async fn independently_authorized_owner_does_not_acquire_shared_three_second_timeout() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("owner").await;
    let mut ticket = dispatch.ticket().clone();
    ticket.basis = ExecutionBasisV1::IndependentOwner {
        owner: digest('6'),
        actor: digest('7'),
        target_ancestry: digest('8'),
        capability: ProtectedCapability::Prompt,
    };
    let dispatch = dispatch.input().clone().bind(ticket).unwrap();
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    f.clock.set(30_000);
    std::thread::sleep(std::time::Duration::from_millis(75));
    assert!(!f.runner.cancelled.load(Ordering::SeqCst));
}

#[test]
fn semantic_flag_is_strict_and_never_activated_startup_writes_nothing() {
    for value in [None, Some("false")] {
        assert!(!enabled(value).unwrap());
    }
    assert!(enabled(Some("true")).unwrap());
    for value in [Some("1"), Some("TRUE"), Some(""), Some(" true")] {
        assert!(enabled(value).is_err());
    }
    let root = std::env::temp_dir().join(format!("cma-unactivated-{}", uuid::Uuid::new_v4()));
    let identity = ReceiverKey {
        kind: ReceiverKind::Runtime,
        instance: id("runtime"),
        incarnation: digest('a'),
        boot: id("boot"),
    };
    let receiver = Receiver::with_runner(
        root.clone(),
        identity,
        false,
        Arc::new(UnavailableRunner),
        Arc::new(BootClock),
    )
    .unwrap();
    assert!(receiver.legacy_allowed());
    assert!(receiver.capabilities().is_none());
    assert!(!root.exists());
    assert!(!root.with_extension("activated").exists());
}

#[tokio::test]
async fn activated_corruption_or_a_second_writer_never_selects_legacy() {
    let f = Fixture::new().await;
    let root = f.root.join("receiver");
    assert!(Receiver::with_runner(
        root.clone(),
        f.receiver.identity.clone(),
        false,
        f.runner.clone(),
        f.clock.clone()
    )
    .is_err());
    std::fs::remove_file(root.join("state.json")).unwrap();
    assert!(f.receiver.state().is_err());
    assert!(!f.receiver.legacy_allowed());
    assert!(Receiver::with_runner(
        root,
        f.receiver.identity.clone(),
        false,
        f.runner.clone(),
        f.clock.clone()
    )
    .is_err());
}
