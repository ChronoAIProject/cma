use super::*;

#[tokio::test]
async fn missing_waiter_keeps_each_webhook_receipt_and_replay_does_not_resend() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    drop(waiter);
    let setup = setup(&id, 2, true).await;
    let receiver = Arc::new(Receiver::default());
    *receiver.statuses.lock().unwrap() = vec![200, 403];
    let (delivery, _rx) = lane(&setup, waiters, receiver.clone());
    let mut lease = accepted(&setup).await;
    let frame = recovery_frame("resp_1", 0, true);
    deliver(
        &setup,
        &delivery,
        &mut lease,
        &frame,
        ReplayEffect::NoEffect,
    )
    .await;
    let evidence = lease
        .state
        .delivery
        .as_ref()
        .unwrap()
        .custom
        .as_ref()
        .unwrap();
    assert_eq!(evidence.local, Some(LocalStreamOutcome::Unavailable));
    assert!(matches!(
        evidence.webhooks[0].status,
        WebhookStatus::Settled {
            outcome: DeliveryOutcome::Confirmed { .. }
        }
    ));
    assert!(matches!(
        evidence.webhooks[1].status,
        WebhookStatus::Settled {
            outcome: DeliveryOutcome::Rejected { .. }
        }
    ));
    setup.journal().release(&mut lease).await.unwrap();
    let mut resumed = accepted(&setup).await;
    deliver(
        &setup,
        &delivery,
        &mut resumed,
        &frame,
        ReplayEffect::NoEffect,
    )
    .await;
    assert_eq!(receiver.events.lock().unwrap().len(), 2);
    assert_eq!(cursor(&resumed.state), Some(0));
}

#[tokio::test]
async fn abandoned_local_intent_is_lost_and_never_replays_the_original_frame() {
    for enqueued in [false, true] {
        let waiters = Arc::new(SyncRegistry::default());
        let (id, waiter) = waiters.register().unwrap();
        let setup = setup(&id, 0, true).await;
        let (delivery, _rx) = lane(&setup, waiters.clone(), Arc::default());
        let mut lease = accepted(&setup).await;
        let frame = recovery_frame("resp_1", 0, true);
        setup.server.fault_after(
            setup.target().id.clone(),
            !enqueued,
            true,
            usize::from(enqueued),
        );
        assert!(delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &frame,
                ReplayEffect::NoEffect
            )
            .await
            .is_err());
        assert_eq!(
            stored(&setup).await.delivery.unwrap().custom.unwrap().local,
            Some(LocalStreamOutcome::Pending)
        );
        setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
        let mut resumed = accepted(&setup).await;
        deliver(
            &setup,
            &delivery,
            &mut resumed,
            &frame,
            ReplayEffect::NoEffect,
        )
        .await;
        assert_eq!(
            resumed
                .state
                .delivery
                .as_ref()
                .unwrap()
                .custom
                .as_ref()
                .unwrap()
                .local,
            Some(LocalStreamOutcome::Lost)
        );
        let text = stream(waiter).await;
        assert_eq!(
            text.matches("event: cmaeg.completed").count(),
            usize::from(enqueued)
        );
        if !enqueued {
            assert!(text.contains("sync_delivery_lost"));
        }
        assert_eq!(cursor(&resumed.state), Some(0));
    }
}

#[tokio::test]
async fn local_receipt_before_crash_is_preserved_without_claiming_reader_delivery() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 0, true).await;
    let (delivery, _rx) = lane(&setup, waiters, Arc::default());
    let mut lease = accepted(&setup).await;
    let frame = recovery_frame("resp_1", 0, true);
    setup
        .server
        .fault_after(setup.target().id.clone(), false, true, 2);
    assert!(delivery
        .deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            ReplayEffect::NoEffect
        )
        .await
        .is_err());
    drop(waiter); // A queue receipt cannot prove the client consumed its frame.
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let mut resumed = accepted(&setup).await;
    let (restarted, _rx) = lane(&setup, Arc::default(), Arc::default());
    deliver(
        &setup,
        &restarted,
        &mut resumed,
        &frame,
        ReplayEffect::NoEffect,
    )
    .await;
    assert_eq!(
        resumed
            .state
            .delivery
            .as_ref()
            .unwrap()
            .custom
            .as_ref()
            .unwrap()
            .local,
        Some(LocalStreamOutcome::Buffered)
    );
    assert!(matches!(
        resumed.state.delivery.as_ref().unwrap().status,
        DeliveryStatus::Settled { receipt: None }
    ));
}

#[tokio::test]
async fn abandoned_webhook_intent_is_unknown_without_resend_or_lost_local_receipt() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 1, true).await;
    let receiver = Arc::new(Receiver::default());
    let target = setup.target().id.clone();
    let memory = setup.server.memory.clone();
    *receiver.on_post.lock().unwrap() =
        Some(Box::new(move || {
            memory.fault.lock().unwrap().replace(
                crate::ingress::routing_store::test_server::Fault {
                    id: target,
                    store: false,
                    lose_read: true,
                    skip: 0,
                },
            );
        }));
    let (delivery, _rx) = lane(&setup, waiters, receiver.clone());
    let mut lease = accepted(&setup).await;
    let frame = recovery_frame("resp_1", 0, true);
    assert!(delivery
        .deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            ReplayEffect::NoEffect
        )
        .await
        .is_err());
    assert!(stream(waiter).await.contains("cmaeg.completed"));
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let mut resumed = accepted(&setup).await;
    assert!(matches!(
        deliver(
            &setup,
            &delivery,
            &mut resumed,
            &frame,
            ReplayEffect::NoEffect
        )
        .await,
        FrameDelivery::Unknown
    ));
    let evidence = resumed
        .state
        .delivery
        .as_ref()
        .unwrap()
        .custom
        .as_ref()
        .unwrap();
    assert_eq!(evidence.local, Some(LocalStreamOutcome::Buffered));
    assert!(matches!(
        evidence.webhooks[0].status,
        WebhookStatus::Unknown
    ));
    assert_eq!(receiver.events.lock().unwrap().len(), 1);
    assert_eq!(cursor(&resumed.state), None);
}

#[tokio::test]
async fn confirmed_webhook_survives_lost_final_settlement_without_repeating_either_leg() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 1, true).await;
    let receiver = Arc::new(Receiver::default());
    let target = setup.target().id.clone();
    let memory = setup.server.memory.clone();
    *receiver.on_post.lock().unwrap() =
        Some(Box::new(move || {
            memory.fault.lock().unwrap().replace(
                crate::ingress::routing_store::test_server::Fault {
                    id: target,
                    store: false,
                    lose_read: true,
                    skip: 1,
                },
            );
        }));
    let (delivery, _rx) = lane(&setup, waiters, receiver.clone());
    let mut lease = accepted(&setup).await;
    let frame = recovery_frame("resp_1", 0, true);
    assert!(delivery
        .deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            ReplayEffect::NoEffect
        )
        .await
        .is_err());
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let mut resumed = accepted(&setup).await;
    assert!(matches!(
        deliver(
            &setup,
            &delivery,
            &mut resumed,
            &frame,
            ReplayEffect::NoEffect
        )
        .await,
        FrameDelivery::Settled { receipt: None, .. }
    ));
    assert_eq!(
        stream(waiter)
            .await
            .matches("event: cmaeg.completed")
            .count(),
        1
    );
    assert_eq!(receiver.events.lock().unwrap().len(), 1);
    assert_eq!(cursor(&resumed.state), Some(0));
}
