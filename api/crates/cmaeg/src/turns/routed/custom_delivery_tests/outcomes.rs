use super::*;

#[tokio::test]
async fn opt_in_is_required_and_backpressure_never_becomes_platform_confirmation() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 0, true).await;
    let mut lease = accepted(&setup).await;
    let (disabled, _rx) = super::super::delivery_tests::lane(&setup);
    assert!(matches!(
        disabled
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &recovery_frame("resp_1", 0, true),
                ReplayEffect::NoEffect
            )
            .await,
        Err(ExecutionError::Unsupported)
    ));
    assert!(lease.state.delivery.is_none());
    let (delivery, _rx) = lane(&setup, waiters.clone(), Arc::default());
    for sequence in 0..9 {
        deliver(
            &setup,
            &delivery,
            &mut lease,
            &delta(sequence, "delta"),
            ReplayEffect::Custom(CustomStreamEffect::Delta {
                text: "delta".into(),
            }),
        )
        .await;
    }
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
        Some(LocalStreamOutcome::Unavailable)
    );
    assert!(matches!(
        lease.state.delivery.as_ref().unwrap().status,
        DeliveryStatus::Settled { receipt: None }
    ));
    assert!(waiters.is_empty());
    assert!(stream(waiter).await.contains("sync_backpressure"));
    assert_eq!(cursor(&lease.state), Some(8));
}

#[tokio::test]
async fn failed_terminal_has_independent_local_and_webhook_failure_envelopes() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 1, false).await;
    let receiver = Arc::new(Receiver::default());
    let (delivery, _rx) = lane(&setup, waiters, receiver.clone());
    let mut lease = accepted(&setup).await;
    let frame = frame(
        json!({"type":"response.incomplete", "sequence_number":0, "response":{"id":"resp_1","incomplete_details":{"reason":"max_output_tokens"}}}),
    );
    deliver(
        &setup,
        &delivery,
        &mut lease,
        &frame,
        ReplayEffect::NoEffect,
    )
    .await;
    assert!(stream(waiter).await.contains("event: cmaeg.failed"));
    assert_eq!(
        receiver.events.lock().unwrap()[0]["type"],
        "cmaeg.response.failed"
    );
}

#[tokio::test]
async fn sync_tools_never_return_unsupported_after_a_local_side_effect() {
    for webhooks in [0, 1] {
        let waiters = Arc::new(SyncRegistry::default());
        let (id, waiter) = waiters.register().unwrap();
        let setup = setup(&id, webhooks, true).await;
        Arc::get_mut(&mut setup.current.channel.lock().unwrap())
            .unwrap()
            .consumer_keys = vec![(setup.target().launch.consumer.clone(), [9; 32])];
        let mut lease = accepted(&setup).await;
        let body = setup.journal().body(&lease).await.unwrap();
        let (delivery, mut rx) = lane(&setup, waiters.clone(), Arc::default());
        let current = setup.current.channel.lock().unwrap().clone();
        let caller = crate::auth::AgentCaller { key_hash: [9; 32] };
        let ReplayEffect::Reply(message) = super::super::delivery_tests::effect() else {
            unreachable!()
        };
        let tools = delivery.tool_replies();
        let origin = event(&body);
        let call = tools.reply_with_idempotency(
            caller,
            &current,
            &origin,
            *message,
            false,
            Some("sync-tool-fixture"),
        );
        let receipt = if webhooks == 0 {
            call.await.unwrap().unwrap()
        } else {
            tokio::join!(call, async {
                let reply = super::super::delivery_tests::command(&mut rx).await;
                assert!(matches!(reply.event.reply, ReplyAddress::None));
                reply
                    .receipt
                    .send(crate::delivery::durable::receipt_for(
                        DeliveryOutcome::Confirmed {
                            platform_message_id: None,
                        },
                    ))
                    .unwrap();
            })
            .0
            .unwrap()
            .unwrap()
        };
        assert_eq!(
            receipt.delivery,
            if webhooks == 0 {
                DeliveryOutcome::Unsupported
            } else {
                DeliveryOutcome::Confirmed {
                    platform_message_id: None,
                }
            }
        );
        deliver(
            &setup,
            &delivery,
            &mut lease,
            &recovery_frame("resp_1", 0, true),
            ReplayEffect::NoEffect,
        )
        .await;
        let text = stream(waiter).await;
        assert!(!text.contains("event: cmaeg.message"));
        assert!(text.contains("event: cmaeg.completed"));
    }
}

#[tokio::test]
async fn webhook_retry_rechecks_frozen_destination_even_if_resolver_reuses_version() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 1, true).await;
    let receiver = Arc::new(Receiver::default());
    *receiver.statuses.lock().unwrap() = vec![500];
    let authority = setup.current.clone();
    *receiver.on_post.lock().unwrap() = Some(Box::new(move || {
        let mut current = authority.channel.lock().unwrap();
        let mut definition = current.definition.clone();
        definition
            .reply
            .sinks
            .retain(|sink| matches!(sink, cmaeg_core::channel::Sink::Sync {}));
        *current = Arc::new(crate::channels::resolver::ResolvedChannel {
            id: current.id.clone(),
            pointer: current.pointer.clone(),
            definition,
            agent_key_hash: current.agent_key_hash,
            consumer_keys: current.consumer_keys.clone(),
            loaded_at: current.loaded_at,
            version: current.version,
        });
    }));
    let (delivery, _rx) = lane(&setup, waiters, receiver.clone());
    let mut lease = accepted(&setup).await;
    deliver(
        &setup,
        &delivery,
        &mut lease,
        &recovery_frame("resp_1", 0, true),
        ReplayEffect::NoEffect,
    )
    .await;
    assert_eq!(
        receiver.events.lock().unwrap().len(),
        1,
        "revoked frozen sink must not be retried"
    );
    assert!(matches!(
        lease
            .state
            .delivery
            .as_ref()
            .unwrap()
            .custom
            .as_ref()
            .unwrap()
            .webhooks[0]
            .status,
        WebhookStatus::Settled {
            outcome: DeliveryOutcome::Rejected { .. }
        }
    ));
    assert!(stream(waiter).await.contains("event: cmaeg.completed"));
}
