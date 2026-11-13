use super::*;

#[tokio::test]
async fn expired_accepted_custom_closes_the_original_stream_without_extending_reply_authority() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 1, true).await;
    let mut lease = accepted(&setup).await;
    setup.journal().release(&mut lease).await.unwrap();
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 1801, Ordering::SeqCst);
    let DispatchOutcome::Replay {
        mut lease, body, ..
    } = setup.dispatch().await.unwrap()
    else {
        panic!("retained accepted response");
    };
    assert!(matches!(
        body.event_context
            .event_ref
            .open(&fixture::keys(), fixture::NOW + 1801),
        Err(cmaeg_core::seal::OpenError::Expired)
    ));
    let receiver = Arc::new(Receiver::default());
    let (delivery, mut platform) = lane(&setup, waiters.clone(), receiver.clone());
    let frame = frame(json!({"type":"response.completed","sequence_number":0,
        "response":{"id":"resp_1","output":[]}}));
    let result = deliver(
        &setup,
        &delivery,
        &mut lease,
        &frame,
        ReplayEffect::Custom(CustomStreamEffect::Completed {
            final_text: Some("expired answer sentinel".into()),
        }),
    )
    .await;
    assert!(matches!(result, FrameDelivery::Settled { cursor: 0, .. }));
    setup
        .journal()
        .complete(&mut lease, "resp_1")
        .await
        .unwrap();
    let evidence = lease
        .state
        .delivery
        .as_ref()
        .unwrap()
        .custom
        .as_ref()
        .unwrap();
    assert_eq!(evidence.local, Some(LocalStreamOutcome::Buffered));
    assert!(
        matches!(&evidence.webhooks[0].status, WebhookStatus::Settled {
        outcome: DeliveryOutcome::Rejected { reason }
    } if reason == "event_ref_expired")
    );
    let text = stream(waiter).await;
    assert_eq!(text.matches("event: cmaeg.completed").count(), 1);
    assert!(!text.contains("expired answer sentinel"));
    assert!(waiters.is_empty());
    assert!(receiver.events.lock().unwrap().is_empty());
    assert!(platform.try_recv().is_err());
    assert_eq!(setup.port.starts.lock().unwrap().len(), 1);
    setup.journal().release(&mut lease).await.unwrap();
    assert!(matches!(
        setup.dispatch().await.unwrap(),
        DispatchOutcome::Terminal(_)
    ));
}

#[tokio::test]
async fn retained_settlement_still_rejects_a_reference_from_a_different_admitted_event() {
    let setup = setup("waiter", 0, false).await;
    let mut lease = accepted(&setup).await;
    let mut body = setup.journal().body(&lease).await.unwrap();
    setup.journal().release(&mut lease).await.unwrap();
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 1801, Ordering::SeqCst);
    let DispatchOutcome::Replay { lease, .. } = setup.dispatch().await.unwrap() else {
        panic!("replay");
    };
    let mut foreign = event(&body);
    foreign.event_id = cmaeg_core::ids::EventId::new("another-event").unwrap();
    body.event_context.event_ref =
        cmaeg_core::seal::EventRef::seal(&foreign, &fixture::keys()).unwrap();
    let (delivery, _) = lane(
        &setup,
        Arc::new(SyncRegistry::default()),
        Arc::new(Receiver::default()),
    );
    assert!(matches!(
        delivery
            .tool_reply_status(&setup.plan.identity, &setup.target().id, &lease, &body)
            .await,
        Err(ExecutionError::Invalid)
    ));
}
