use super::*;

#[tokio::test]
async fn automatic_reservation_blocks_tool_and_terminal_close_refuses_late_admission() {
    let setup = Setup::new(supported()).await;
    let caller = caller(&setup);
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let event = event(&body);
    let channel = setup.current.channel.lock().unwrap().clone();
    let (delivery, mut rx) = lane(&setup);
    let tools = delivery.tool_replies();
    let frame = recovery_frame("resp_1", 0, true);
    let (result, ()) = tokio::join!(
        delivery.deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            effect(),
        ),
        async {
            let reply = command(&mut rx).await;
            assert!(matches!(
                tools
                    .reply(caller, &channel, &event, message(), false)
                    .await,
                Err(ExecutionError::Busy)
            ));
            reply.receipt.send(receipt()).unwrap();
        }
    );
    result.unwrap();
    assert!(matches!(
        tools
            .reply(caller, &channel, &event, message(), false)
            .await,
        Err(ExecutionError::Invalid)
    ));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn routed_reference_never_falls_back_after_routing_or_owner_revocation() {
    for owner_changed in [false, true] {
        let setup = Setup::new(supported()).await;
        let caller = caller(&setup);
        let lease = accepted(&setup).await;
        let body = setup.journal().body(&lease).await.unwrap();
        let event = event(&body);
        {
            let mut current = setup.current.channel.lock().unwrap();
            let definition = &mut Arc::get_mut(&mut current).unwrap().definition;
            if owner_changed {
                definition.owner.subject = "different-owner".into();
            } else {
                definition.routing = None;
            }
        }
        let channel = setup.current.channel.lock().unwrap().clone();
        let (delivery, mut rx) = lane(&setup);
        assert!(matches!(
            delivery
                .tool_replies()
                .reply(caller, &channel, &event, message(), false,)
                .await,
            Err(ExecutionError::AuthorityChanged)
        ));
        assert!(rx.try_recv().is_err());
        setup.server.memory.clock.fetch_add(901, Ordering::SeqCst);
        assert!(matches!(
            delivery
                .tool_replies()
                .reply(caller, &channel, &event, message(), true)
                .await,
            Err(ExecutionError::AuthorityChanged)
        ));
    }
}

#[tokio::test]
async fn explicit_failure_reply_can_settle_when_automatic_final_answers_are_disabled() {
    let setup = Setup::new(supported()).await;
    {
        let mut current = setup.current.channel.lock().unwrap();
        let definition = &mut Arc::get_mut(&mut current).unwrap().definition;
        for consumer in &mut definition.routing.as_mut().unwrap().consumers {
            consumer.reply.auto_final = Some(false);
        }
    }
    let mut lease = accepted(&setup).await;
    assert!(
        !setup
            .journal()
            .body(&lease)
            .await
            .unwrap()
            .delivery
            .auto_final
    );
    let (delivery, mut rx) = lane(&setup);
    let raw = serde_json::json!({"type":"response.failed", "response_id":"resp_1",
        "sequence_number":0, "error":{"code":"provider_failed","message":"failed"}});
    let frame = crate::providers::RecoveryEvent {
        response_id: "resp_1".into(),
        sequence_number: 0,
        event: crate::providers::parse_event(&crate::providers::SseFrame {
            event: None,
            id: Some("0".into()),
            data: raw.to_string(),
            retry: None,
        })
        .unwrap(),
        raw,
    };
    let (result, ()) = tokio::join!(
        delivery.deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            effect()
        ),
        async {
            command(&mut rx).await.receipt.send(receipt()).unwrap();
        }
    );
    assert!(matches!(
        result.unwrap(),
        FrameDelivery::Settled {
            receipt: Some(_),
            ..
        }
    ));
    setup
        .journal()
        .complete(&mut lease, "resp_1")
        .await
        .unwrap();
}
