use super::{test_support::*, *};
use std::sync::atomic::Ordering;

#[tokio::test]
async fn first_replay_includes_event_zero_until_its_delivery_is_acknowledged() {
    let setup = Setup::new(supported()).await;
    let DispatchOutcome::Replay {
        mut lease,
        response_id,
        durable_cursor,
        ..
    } = setup.dispatch().await.unwrap()
    else {
        panic!("accepted")
    };
    assert_eq!(durable_cursor, None);
    setup.journal().release(&mut lease).await.unwrap();
    let DispatchOutcome::Replay {
        mut lease,
        durable_cursor,
        ..
    } = setup.dispatch().await.unwrap()
    else {
        panic!("replay")
    };
    assert_eq!(durable_cursor, None);
    acknowledge_no_effect(&setup, &mut lease, &response_id, 0, false).await;
    setup.journal().release(&mut lease).await.unwrap();
    let DispatchOutcome::Replay { durable_cursor, .. } = setup.dispatch().await.unwrap() else {
        panic!("replay")
    };
    assert_eq!(durable_cursor, Some(0));
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn interrupted_start_stays_unknown_without_proven_exact_body_idempotency() {
    let setup = Setup::new(RecoverySupport {
        exact_body_idempotency: false,
        response_replay: true,
    })
    .await;
    let mut lease = setup.prepared().await;
    setup
        .journal()
        .transition(&mut lease, Phase::Starting)
        .await
        .unwrap();
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Unknown
    ));
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Unknown
    ));
    assert!(setup.port.starts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn accepted_restart_replays_original_response_and_cursor_without_posting_again() {
    let setup = Setup::new(supported()).await;
    let DispatchOutcome::Replay {
        mut lease,
        body,
        response_id,
        ..
    } = setup.dispatch().await.unwrap()
    else {
        panic!("accepted")
    };
    assert_eq!(
        setup
            .journal()
            .acknowledge_delivery(&mut lease, "other", 1)
            .await,
        Err(ExecutionError::Invalid)
    );
    acknowledge_no_effect(&setup, &mut lease, &response_id, 7, true).await;
    assert_eq!(
        setup
            .journal()
            .acknowledge_delivery(&mut lease, &response_id, 6)
            .await,
        Err(ExecutionError::Invalid)
    );
    // Provider reads may finish after the event's original delivery authority
    // has expired. Nothing extends or reseals that reference.
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 901, Ordering::SeqCst);
    let restarted = Dispatcher::new(
        setup.server.store(),
        setup.journal().clone(),
        setup.current.clone(),
        setup.port.clone(),
    );
    let DispatchOutcome::Replay {
        mut lease,
        body: replayed,
        response_id: restored,
        durable_cursor,
    } = restarted
        .dispatch(&setup.plan.identity, &setup.target().id, "restarted")
        .await
        .unwrap()
    else {
        panic!("replay")
    };
    assert_eq!(restored, response_id);
    assert_eq!(durable_cursor, Some(7));
    assert_eq!(
        super::storage::bytes(&body).unwrap(),
        super::storage::bytes(&replayed).unwrap()
    );
    assert_eq!(event(&replayed).expires_at, fixture::NOW + 900);
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
    setup
        .journal()
        .complete(&mut lease, &response_id)
        .await
        .unwrap();
    setup.journal().release(&mut lease).await.unwrap();
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Terminal(Phase::Completed {
            durable_cursor: Some(7),
            ..
        })
    ));
}

#[tokio::test]
async fn sync_only_primary_keeps_original_waiter() {
    let mut request = fixture::request(fixture::targets, fixture::custom(None));
    fixture::rewrite_ref(&mut request, |event| {
        event.reply = cmaeg_core::seal::ReplyAddress::Sync {
            waiter: "sync-fixture".into(),
        }
    });
    let setup = Setup::with_request(supported(), request).await;
    for (index, target) in setup.plan.targets.iter().enumerate() {
        let body = frozen(
            &setup.plan,
            target,
            &setup.current.channel.lock().unwrap(),
            1,
        );
        if index == 0 {
            assert!(matches!(
                event(&body).reply,
                cmaeg_core::seal::ReplyAddress::Sync { .. }
            ));
        } else {
            assert!(matches!(
                event(&body).reply,
                cmaeg_core::seal::ReplyAddress::None
            ));
        }
    }
}

#[tokio::test]
async fn known_nonacceptance_retries_same_frozen_request_and_rejection_is_terminal() {
    let setup = Setup::new(supported()).await;
    *setup.port.next.lock().unwrap() = Some(StartResult::NotAccepted);
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Retry
    ));
    *setup.port.next.lock().unwrap() = Some(StartResult::Rejected {
        code: "forbidden".into(),
    });
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Terminal(Phase::Refused { .. })
    ));
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Terminal(Phase::Refused { .. })
    ));
    let starts = setup.port.starts.lock().unwrap();
    assert_eq!(starts.len(), 2);
    assert_eq!(starts[0].0, starts[1].0);
    assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_target_not_in_the_committed_plan_is_never_started() {
    let setup = Setup::new(supported()).await;
    assert!(matches!(
        setup
            .dispatcher
            .dispatch(&setup.plan.identity, "foreign-target", "worker")
            .await,
        Err(ExecutionError::Invalid)
    ));
    assert!(setup.port.starts.lock().unwrap().is_empty());
}
