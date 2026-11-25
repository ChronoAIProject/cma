use super::{
    delivery_tests::{command, lane, stored},
    executor_tests::{executor, frames},
    scheduler::{Disposition, TargetExecutor},
    test_support::*,
    tool_reply_store::{Admission, Kind, ReplyStore},
    *,
};
use crate::{
    auth::AgentCaller,
    delivery::{durable::receipt_for, intents::DeliveryOutcome},
};
use std::sync::Arc;

#[tokio::test]
async fn restarted_executor_uses_durable_tool_receipt_without_process_tracker() {
    let setup = Setup::new(supported()).await;
    {
        let mut current = setup.current.channel.lock().unwrap();
        Arc::get_mut(&mut current).unwrap().consumer_keys =
            vec![(setup.target().launch.consumer.clone(), [9; 32])];
    }
    let http = super::adapter_test_server::HttpServer::start().await;
    http.memory.lock().unwrap().config.replay_frames = Some(frames());
    let (mut first, _rx, work) = executor(&setup, &http);
    first.frame_limit = 1;
    assert!(matches!(first.execute(&work).await, Disposition::Retry(_)));
    let mut lease = setup
        .journal()
        .claim(&setup.plan, setup.target(), "tool-fixture")
        .await
        .unwrap();
    let body = setup.journal().body(&lease).await.unwrap();
    let event = event(&body);
    setup.journal().release(&mut lease).await.unwrap();
    let channel = setup.current.channel.lock().unwrap().clone();
    let (delivery, mut rx) = lane(&setup);
    let tools = delivery.tool_replies();
    let ReplayEffect::Reply(message) = super::delivery_tests::effect() else {
        unreachable!();
    };
    let (result, ()) = tokio::join!(
        tools.reply(
            AgentCaller { key_hash: [9; 32] },
            &channel,
            &event,
            *message,
            false
        ),
        async {
            let reply = command(&mut rx).await;
            let _permit = reply.gate.attempt().await.unwrap();
            assert!(reply.gate.commit());
            reply
                .receipt
                .send(receipt_for(DeliveryOutcome::Confirmed {
                    platform_message_id: Some("tool-answer".into()),
                }))
                .unwrap();
        }
    );
    result.unwrap();
    drop(first);
    let (resumed, mut rx, work) = executor(&setup, &http);
    assert!(matches!(resumed.execute(&work).await, Disposition::Settled));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Completed {
            durable_cursor: Some(3),
            ..
        }
    ));
    assert!(
        rx.try_recv().is_err(),
        "confirmed tool receipt suppresses automatic fallback after restart"
    );
    assert_eq!(
        http.seen()
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        1
    );
}

#[tokio::test]
async fn pending_then_unknown_tool_delivery_prevents_terminal_completion_without_public_text() {
    let setup = Setup::new(supported()).await;
    let http = super::adapter_test_server::HttpServer::start().await;
    // Empty final output means no automatic public message is needed, but an
    // unresolved independent tool delivery must still prevent completion.
    let (mut runner, mut rx, work) = executor(&setup, &http);
    runner.frame_limit = 1;
    assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
    let store = ReplyStore(setup.journal().clone(), None);
    let Admission::Claimed(claim) = store
        .admit(&setup.plan, setup.target(), Kind::Tool, "a".repeat(64))
        .await
        .unwrap()
    else {
        panic!("claim");
    };
    runner.frame_limit = 4096;
    assert!(matches!(runner.execute(&work).await, Disposition::Retry(_)));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Accepted {
            durable_cursor: Some(1),
            ..
        }
    ));
    store
        .finish(
            &claim,
            &receipt_for(DeliveryOutcome::Unknown {
                reason: "receipt_lost".into(),
            }),
        )
        .await
        .unwrap();
    assert!(matches!(runner.execute(&work).await, Disposition::Settled));
    assert!(matches!(
        stored(&setup).await.phase,
        Phase::Accepted {
            durable_cursor: Some(1),
            ..
        }
    ));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn event_side_message_receipt_never_suppresses_the_restarted_executor_answer() {
    use crate::tools::{reply_targets::receipts, side_messages::claim_side_message};

    for outcome in [
        None,
        Some(DeliveryOutcome::Confirmed {
            platform_message_id: Some("side-delivered".into()),
        }),
        Some(DeliveryOutcome::Rejected {
            reason: "side_refused".into(),
        }),
        Some(DeliveryOutcome::Unknown {
            reason: "side_unknown".into(),
        }),
    ] {
        let setup = Setup::new(supported()).await;
        let http = super::adapter_test_server::HttpServer::start().await;
        http.memory.lock().unwrap().config.replay_frames = Some(frames());
        let (mut first, mut rx, work) = executor(&setup, &http);
        first.frame_limit = 2;
        assert!(matches!(first.execute(&work).await, Disposition::Retry(_)));
        assert!(rx.try_recv().is_err());

        let journal = setup.journal();
        let mut lease = journal
            .claim(&setup.plan, setup.target(), "side-fixture")
            .await
            .unwrap();
        let payload = event(&journal.body(&lease).await.unwrap());
        journal.release(&mut lease).await.unwrap();
        let store = receipts::Store::new(journal.records.clone(), journal.keys.clone());
        let ReplayEffect::Reply(message) = super::delivery_tests::effect() else {
            unreachable!();
        };
        let receipts::Admission::Claimed(claim) = claim_side_message(
            &store,
            &payload,
            "side-message-1",
            &message,
            &setup.plan.identity.owner,
        )
        .await
        .unwrap() else {
            panic!("first side message must claim its own receipt");
        };
        if let Some(outcome) = outcome {
            store.finish(claim, &receipt_for(outcome)).await.unwrap();
        }
        drop(first);

        let (resumed, mut rx, work) = executor(&setup, &http);
        let (result, ()) = tokio::join!(resumed.execute(&work), async {
            let reply = command(&mut rx).await;
            assert_eq!(reply.message.text.as_deref(), Some("hello world"));
            reply
                .receipt
                .send(receipt_for(DeliveryOutcome::Confirmed {
                    platform_message_id: Some("final-reply".into()),
                }))
                .unwrap();
        });
        assert!(matches!(result, Disposition::Settled));
        assert!(matches!(
            stored(&setup).await.phase,
            Phase::Completed {
                durable_cursor: Some(3),
                ..
            }
        ));
        assert!(matches!(resumed.execute(&work).await, Disposition::Settled));
        assert!(rx.try_recv().is_err());
    }
}

#[tokio::test]
async fn event_side_message_routed_authority_refuses_waiting_send_after_quarantine_rebind_or_rotation(
) {
    use crate::{
        channels::resolver::ChannelResolver,
        delivery::{engine::DeliveryEngine, relay_client::RelayClient},
        ingress::routing_store::test_server::{Row, Server},
        telegram::media_fixture::Fixture,
        tools::side_messages::SideMessageGate,
    };
    use cmaeg_core::{ids::ConversationId, seal::seal_blob};
    use std::time::Duration;

    for change in ["quarantine", "provider rebind", "credential rotation"] {
        let channel_store = Server::start().await;
        let mut request = fixture::request(
            |value| {
                value["record_id"] = uuid::Uuid::new_v4().to_string().into();
                value["routing"]["default_targets"] = serde_json::json!(["default"]);
                value["secrets"]["telegram_bot_token"] = "77:fixture".into();
            },
            fixture::message("original"),
        );
        let keys = Arc::new(fixture::keys());
        let channel = Arc::make_mut(&mut request.channel);
        channel.pointer.record_url = channel_store
            .records
            .record_url(channel.pointer.record_id.as_str())
            .unwrap()
            .to_string();
        channel.id = channel.pointer.seal_id(&keys).unwrap();
        let channel_id = channel.id.clone();
        let record_id = channel.pointer.record_id.clone();
        let conversation_id = ConversationId::derive(
            &keys,
            &channel_id,
            1,
            &request
                .activity
                .partition_key(&channel.definition.partition),
        );
        channel_store.memory.channels.lock().unwrap().insert(
            channel.pointer.record_id.to_string(),
            Row {
                owner: channel.definition.owner.subject.clone(),
                blob: seal_blob(
                    &keys,
                    channel.definition.to_blob_json().to_string().as_bytes(),
                )
                .unwrap(),
                expiry: i64::MAX,
                version: 1,
            },
        );
        request.conversation_id = conversation_id.clone();
        fixture::rewrite_ref(&mut request, |event| {
            event.channel_id = channel_id.clone();
            event.record_id = record_id;
            event.conversation_id = conversation_id;
        });
        let resolver = ChannelResolver::new(
            Arc::new(channel_store.records.clone()),
            keys.clone(),
            Default::default(),
        );
        request.channel = resolver.resolve_current(channel_id.as_str()).await.unwrap();
        let setup = Setup::with_server(supported(), request, b"raw-event", channel_store).await;
        let lease = super::delivery_tests::accepted(&setup).await;
        let payload = event(&setup.journal().body(&lease).await.unwrap());
        let channel = setup.current.channel.lock().unwrap().clone();
        let caller = AgentCaller {
            key_hash: *channel
                .consumer_key_hash(payload.consumer.as_ref())
                .unwrap(),
        };
        let current: Arc<dyn Authority> = if change == "quarantine" {
            Arc::new(resolver.clone())
        } else {
            setup.current.clone()
        };
        // The reader has the backing records and works without a recovery runtime.
        let authority =
            SideMessageAuthority::new(resolver.record_client(), keys.clone(), current, false);
        authority
            .check(caller, &payload, channel.version)
            .await
            .unwrap();

        let platform = Fixture::new().await;
        let relay = RelayClient::new("http://127.0.0.1:1", Duration::from_secs(1))
            .unwrap()
            .with_telegram(platform.api.clone());
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        let engine = DeliveryEngine::start(
            rx,
            keys,
            relay,
            crate::DeliveryConfig::default(),
            crate::turns::TurnTracker::new(Default::default()),
            crate::delivery::custom::CustomDelivery::new(
                "https://gateway.example".into(),
                Arc::default(),
            ),
        );
        let delivery = engine.side_messages_if(true).unwrap();
        let gate = SideMessageGate::new(
            resolver.clone(),
            channel.clone(),
            payload.clone(),
            caller,
            authority,
        );
        let message: cmaeg_core::message::OutboundMessage =
            serde_json::from_value(serde_json::json!({"text":"progress"})).unwrap();
        let mut send = Box::pin(delivery.send(&channel, &payload, &message, &gate));
        assert!(futures_util::poll!(&mut send).is_pending());
        if change == "quarantine" {
            let saved = super::quarantine::Quarantine {
                kind: super::quarantine::KIND.into(),
                target: setup.target().id.clone(),
                root: setup.plan.admission_id.clone(),
                owner: setup.plan.identity.owner.clone(),
                request_hash: setup.plan.request_hash.clone(),
                retained_until: setup.plan.retained_until,
                audit: "fixture-audit".into(),
                rollback_safe_after: payload
                    .expires_at
                    .saturating_add(cmaeg_core::seal::REPLY_TARGET_TTL),
            };
            setup
                .journal()
                .write(
                    &super::quarantine::id(&saved.target),
                    &saved.owner,
                    saved.retained_until,
                    &saved,
                    None,
                )
                .await
                .unwrap();
            assert!(matches!(
                SideMessageAuthority::new(
                    resolver.record_client(),
                    Arc::new(fixture::keys()),
                    Arc::new(resolver),
                    false,
                )
                .check(caller, &payload, channel.version)
                .await,
                Err(ExecutionError::Abandoned)
            ));
        } else {
            let mut current = setup.current.channel.lock().unwrap();
            let current = Arc::make_mut(&mut current);
            if change == "provider rebind" {
                crate::channels::definition::fixture::replace_default_binding(
                    &mut current.definition,
                    "replacement-binding".parse().unwrap(),
                );
            } else {
                current.version += 1;
                current.definition.secrets.telegram_bot_token =
                    Some(cmaeg_core::channel::Secret::new("77:rotated-fixture"));
            }
        }
        let result = send.await;
        assert!(
            matches!(result.delivery, DeliveryOutcome::Rejected { ref reason } if reason == "side_message_authority_changed"),
            "{change}: {:?}",
            result.delivery
        );
        assert!(platform.calls().is_empty());
    }
}
