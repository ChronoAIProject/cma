use super::*;

#[tokio::test]
async fn expired_executor_closes_only_original_primary_stream_without_content_or_external_sends() {
    use super::super::{
        adapter_test_server::HttpServer,
        scheduler::{Disposition, TargetExecutor, WorkItem},
    };
    use crate::{http::readiness::Readiness, turns::tracker::TurnTracker, DeliveryConfig};
    use std::time::Duration;

    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 1, true).await;
    let receiver = Arc::new(Receiver::default());
    let http = HttpServer::start().await;
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 900, Ordering::SeqCst);
    setup.current.revoke_at.store(0, Ordering::SeqCst);
    let make_executor = || {
        let port = Arc::new(HttpExecutionPort::new(
            http.client(true, Duration::from_secs(60)),
            setup.port.clone(),
            setup.server.store(),
            setup.journal().clone(),
            setup.current.clone(),
        ));
        let (delivery, rx) = lane(&setup, waiters.clone(), receiver.clone());
        (
            RoutedTargetExecutor::new(
                port,
                delivery,
                TurnTracker::new(Readiness::initialized()),
                &DeliveryConfig::default(),
            ),
            rx,
        )
    };
    let (runner, mut rx) = make_executor();
    assert!(setup.plan.targets.len() > 1);
    let secondary = WorkItem {
        identity: setup.plan.identity.clone(),
        target_id: setup.plan.targets[1].id.clone(),
    };
    assert!(matches!(
        runner.execute(&secondary).await,
        Disposition::Settled
    ));
    assert_eq!(waiters.len(), 1);
    let primary = WorkItem {
        identity: setup.plan.identity.clone(),
        target_id: setup.target().id.clone(),
    };
    assert!(matches!(
        runner.execute(&primary).await,
        Disposition::Settled
    ));
    assert!(waiters.is_empty());
    let (restarted, mut restarted_rx) = make_executor();
    assert!(matches!(
        restarted.execute(&primary).await,
        Disposition::Settled
    ));
    let stream = stream(waiter).await;
    assert_eq!(stream.matches("event: cmaeg.failed").count(), 1);
    let data: Value = serde_json::from_str(
        stream
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(data["event_id"], json!(setup.plan.identity.event_id));
    assert_eq!(
        data["conversation_id"],
        json!(setup.target().launch.event.conversation_id)
    );
    assert!(data["text"].is_null());
    assert!(data["message"].is_null());
    assert_eq!(data["error"]["code"], "event_ref_expired");
    assert_eq!(data["error"]["status"], 410);
    assert_eq!(
        data["error"]["detail"],
        super::super::expiry::EXPIRED_BEFORE_START
    );
    assert!(http.seen().is_empty());
    assert!(receiver.events.lock().unwrap().is_empty());
    assert!(rx.try_recv().is_err());
    assert!(restarted_rx.try_recv().is_err());
    assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 0);
    assert_eq!(setup.current.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_terminal_is_independent_of_auto_final_and_tool_suppression() {
    for (auto_final, tool_replied) in [(false, false), (true, true)] {
        let waiters = Arc::new(SyncRegistry::default());
        let (id, waiter) = waiters.register().unwrap();
        let setup = setup(&id, 1, auto_final).await;
        let mut lease = accepted(&setup).await;
        let body = setup.journal().body(&lease).await.unwrap();
        let receiver = Arc::new(Receiver::default());
        let (delivery, _rx) = lane(&setup, waiters.clone(), receiver.clone());
        if tool_replied {
            let store = super::super::tool_reply_store::ReplyStore(setup.journal().clone());
            let super::super::tool_reply_store::Admission::Claimed(claim) = store
                .admit(
                    &setup.plan,
                    setup.target(),
                    super::super::tool_reply_store::Kind::Tool,
                    "f".repeat(64),
                )
                .await
                .unwrap()
            else {
                panic!("claimed")
            };
            store
                .finish(
                    &claim,
                    &crate::delivery::durable::receipt_for(DeliveryOutcome::Confirmed {
                        platform_message_id: None,
                    }),
                )
                .await
                .unwrap();
        }
        let mut projection = ReplayProjection::new("resp_1".into());
        let frame = frame(
            json!({"type":"response.completed","sequence_number":0,"response":{"id":"resp_1","output":[{"type":"message","text":"suppressed answer"}]}}),
        );
        let effect = projection
            .project(&frame, &body, tool_replied, "failed")
            .unwrap();
        assert!(matches!(
            effect,
            ReplayEffect::Custom(CustomStreamEffect::Completed { final_text: None })
        ));
        assert!(matches!(
            deliver(&setup, &delivery, &mut lease, &frame, effect).await,
            FrameDelivery::Settled { receipt: None, .. }
        ));
        let checkpoint = lease.state.delivery.as_ref().unwrap();
        let evidence = checkpoint.custom.as_ref().unwrap();
        assert_eq!(evidence.local, Some(LocalStreamOutcome::Buffered));
        assert!(matches!(
            evidence.webhooks[0].status,
            WebhookStatus::Settled {
                outcome: DeliveryOutcome::Confirmed { .. }
            }
        ));
        let text = stream(waiter).await;
        assert!(text.contains("cmaeg.completed"));
        assert!(!text.contains("suppressed answer"));
        assert!(waiters.is_empty());
        let events = receiver.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "cmaeg.response.completed");
        assert!(events[0]["data"]["text"].is_null());
    }
}

#[tokio::test]
async fn deltas_and_committed_messages_reach_only_the_primary_waiter() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 0, true).await;
    let receiver = Arc::new(Receiver::default());
    let (delivery, _rx) = lane(&setup, waiters.clone(), receiver);
    let secondary = &setup.plan.targets[1];
    let DispatchOutcome::Replay { mut lease, .. } = setup
        .dispatcher
        .dispatch(&setup.plan.identity, &secondary.id, "secondary")
        .await
        .unwrap()
    else {
        panic!("accepted")
    };
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &secondary.id,
                &mut lease,
                &recovery_frame("resp_1", 0, true),
                ReplayEffect::NoEffect
            )
            .await
            .unwrap(),
        FrameDelivery::Settled { .. }
    ));
    assert_eq!(
        lease
            .state
            .delivery
            .as_ref()
            .unwrap()
            .custom
            .as_ref()
            .unwrap()
            .local,
        None
    );
    assert_eq!(
        waiters.len(),
        1,
        "secondary terminal must not close primary"
    );
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let mut projection = ReplayProjection::new("resp_1".into());
    for frame in [
        delta(0, "hello"),
        frame(
            json!({"type":"response.output_text.done","sequence_number":1,"item_id":"message","text":"hello"}),
        ),
        recovery_frame("resp_1", 2, true),
    ] {
        let effect = projection.project(&frame, &body, false, "failed").unwrap();
        assert!(matches!(
            deliver(&setup, &delivery, &mut lease, &frame, effect).await,
            FrameDelivery::Settled { receipt: None, .. }
        ));
    }
    let text = stream(waiter).await;
    assert_eq!(text.matches("event: cmaeg.text.delta").count(), 1);
    assert_eq!(text.matches("event: cmaeg.message").count(), 1);
    assert_eq!(text.matches("event: cmaeg.completed").count(), 1);
    assert_eq!(cursor(&lease.state), Some(2));
}
