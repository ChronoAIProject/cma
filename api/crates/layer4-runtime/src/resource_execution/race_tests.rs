use super::*;

#[tokio::test]
async fn dropped_start_waiter_cannot_strand_the_preparation_owner_or_allow_late_entry() {
    let f = Fixture::new().await;
    f.runner.hold_prepare.store(true, Ordering::SeqCst);
    let dispatch = f.dispatch("first").await;
    let request = f.barrier(&dispatch);
    let receiver = f.receiver.clone();
    let authority = f.authority.clone();
    let waiter = tokio::spawn(async move { receiver.start(authority, dispatch).await });
    f.runner.preparing.notified().await;
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
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
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        match f
            .receiver
            .stop(f.owner.clone(), request.clone(), true)
            .await
        {
            Ok(Some(stop)) => {
                assert_eq!(stop.evidence, StopEvidenceKind::RejectedBeforeStart);
                break;
            }
            Err(ProtocolError::Unknown) => {
                assert!(tokio::time::Instant::now() < until);
                tokio::task::yield_now().await;
            }
            other => panic!("unexpected stop observation: {other:?}"),
        }
    }
    assert_eq!(f.runner.entries.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn runner_cannot_contradict_possible_native_entry_with_no_entry_evidence() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let request = f.barrier(&dispatch);
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    f.runner.reports_no_entry.store(true, Ordering::SeqCst);
    assert_eq!(
        f.receiver
            .stop(f.owner.clone(), request.clone(), true)
            .await
            .unwrap_err(),
        ProtocolError::EvidenceMismatch
    );
    assert!(f
        .receiver
        .stop(f.owner.clone(), request, false)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn family_barrier_closes_siblings_but_preserves_a_fresh_grant_and_owner_gate() {
    let f = Fixture::new().await;
    let first = f.dispatch("first").await;
    let sibling = f.dispatch("sibling").await;
    let owner = f.dispatch("owner").await;
    let mut owner_ticket = owner.ticket().clone();
    owner_ticket.basis = ExecutionBasisV1::IndependentOwner {
        owner: digest('6'),
        actor: digest('7'),
        target_ancestry: digest('8'),
        capability: ProtectedCapability::Prompt,
    };
    f.receiver
        .start(f.authority.clone(), first.clone())
        .await
        .unwrap();
    f.receiver
        .start(f.authority.clone(), sibling.clone())
        .await
        .unwrap();
    f.receiver
        .start(
            f.authority.clone(),
            owner.input().clone().bind(owner_ticket).unwrap(),
        )
        .await
        .unwrap();
    f.receiver
        .barrier(f.owner.clone(), f.barrier(&first))
        .await
        .unwrap();
    {
        let state = f.receiver.state.lock().unwrap();
        assert!(state.gates[0].ready_to_observe_stop());
        assert!(state.gates[1].ready_to_observe_stop());
        assert!(!state.gates[2].ready_to_observe_stop());
    }
    let late = f.dispatch("late-old-grant").await;
    assert_eq!(
        f.receiver
            .start(f.authority.clone(), late)
            .await
            .unwrap_err(),
        ProtocolError::Expired
    );
    let fresh = f.dispatch("fresh-grant").await;
    let mut ticket = fresh.ticket().clone();
    let ExecutionBasisV1::Share { grant } = &mut ticket.basis else {
        unreachable!()
    };
    grant.family.generation = generation(2);
    f.receiver
        .start(
            f.authority.clone(),
            fresh.input().clone().bind(ticket).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(f.runner.entries.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn full_admission_history_cannot_block_an_existing_member_barrier_or_stop() {
    let f = Fixture::new().await;
    let first = f.dispatch("first").await;
    for index in 1..journal::MAX_RECORDS {
        f.dispatch(&format!("command-{index}")).await;
    }
    let mut next = first.ticket().execution.clone();
    next.command_id = id("overflow");
    assert_eq!(
        f.receiver
            .challenge(f.owner.clone(), next)
            .await
            .unwrap_err(),
        ProtocolError::Capacity
    );
    let stop = f
        .receiver
        .stop(f.owner.clone(), f.barrier(&first), true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stop.evidence, StopEvidenceKind::RejectedBeforeStart);
    assert_eq!(
        f.receiver
            .state
            .lock()
            .unwrap()
            .snapshot
            .as_ref()
            .unwrap()
            .records
            .len(),
        journal::MAX_RECORDS
    );
}

#[tokio::test]
async fn persisted_continuation_retry_uses_the_original_deadline_and_cannot_reopen_expiry() {
    for expired in [false, true] {
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
        let lease = ContinuationV1 {
            protocol: 1,
            execution: ticket.execution,
            registration: ticket.registration,
            basis: ticket.basis,
            manager: ticket.manager,
            challenge,
            sequence: generation(2),
        };
        // The journal commit may outlive the waiter before volatile renewal.
        {
            let mut state = f.receiver.state().unwrap();
            let mut snapshot = state.snapshot.clone().unwrap();
            snapshot.records[0].continuation = Some(lease.clone());
            snapshot.next().unwrap();
            state.publish(snapshot).unwrap();
        }
        f.clock.set(if expired { 3100 } else { 2100 });
        let result = f
            .receiver
            .continue_execution(f.authority.clone(), lease.clone())
            .await;
        if expired {
            assert_eq!(result.unwrap_err(), ProtocolError::Expired);
        } else {
            result.unwrap();
            f.clock.set(3100);
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
    }
}

#[tokio::test]
async fn failed_forced_termination_is_retried_and_never_becomes_stop_evidence() {
    let f = Fixture::new().await;
    let dispatch = f.dispatch("first").await;
    let request = f.barrier(&dispatch);
    f.receiver
        .start(f.authority.clone(), dispatch)
        .await
        .unwrap();
    f.runner.quiescent.store(false, Ordering::SeqCst);
    f.runner.force_fails.store(true, Ordering::SeqCst);
    f.clock.set(3100);
    wait_thread(|| f.runner.cancelled.load(Ordering::SeqCst));
    f.clock.set(5100);
    wait_thread(|| f.runner.forced.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        f.receiver
            .stop(f.owner.clone(), request.clone(), true)
            .await
            .unwrap_err(),
        ProtocolError::Unknown
    );
    assert!(f
        .receiver
        .stop(f.owner.clone(), request, false)
        .await
        .unwrap()
        .is_none());
}
