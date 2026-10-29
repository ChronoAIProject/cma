//! Exact retained Telegram destinations share the durable keyed reply ledger.
use super::{
    delivery_tests::{accepted, lane},
    test_support::*,
};
use crate::{
    auth::AgentCaller,
    delivery::{
        custom::CustomDelivery, engine::DeliveryEngine, intents::DeliveryOutcome,
        relay_client::RelayClient,
    },
    http::readiness::Readiness,
    interactions::InteractionStore,
    telegram::{
        media_fixture::{Answer, Fixture},
        structured_tests::{result, samples},
    },
    turns::TurnTracker,
    DeliveryConfig,
};
use cmaeg_core::{message::OutboundMessage, seal::ReplyAddress};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

async fn setup(named: bool) -> Setup {
    let mut activity = fixture::message("reply with static facts");
    activity.conversation.id = "-42".into();
    activity.conversation.thread_id = Some("7".into());
    activity.conversation.kind = cmaeg_core::activity::ConversationKind::Group;
    let mut request = fixture::request(
        |value| {
            value["sources"] =
                json!([{"type":"telegram","bot_id":"77","admission":{"type":"all"}}]);
            value["secrets"]["telegram_bot_token"] = "77:fixture".into();
            value["routing"]["default_targets"] =
                json!([if named { fixture::REVIEWER } else { "default" }]);
            value["reply"]["sinks"] = json!([{"type":"source"}]);
        },
        activity,
    );
    fixture::rewrite_ref(&mut request, |event| {
        let ReplyAddress::Telegram { thread_id, .. } = &mut event.reply else {
            panic!()
        };
        *thread_id = Some("7".into());
    });
    let s = Setup::with_request(supported(), request).await;
    {
        let mut channel = s.current.channel.lock().unwrap();
        let channel = Arc::make_mut(&mut channel);
        channel.agent_key_hash = [9; 32];
        channel.consumer_keys = vec![(s.target().launch.consumer.clone(), [9; 32])];
    }
    s
}

#[tokio::test]
async fn static_replies_keep_original_topic_consumer_and_key_across_gate_off_restart_replay() {
    for named in [false, true] {
        for (_, content) in samples() {
            for uncertain in [false, true] {
                let s = setup(named).await;
                let mut lease = accepted(&s).await;
                let event = event(&s.journal().body(&lease).await.unwrap());
                let channel = s.current.channel.lock().unwrap().clone();
                let f = Fixture::new().await;
                f.answer(Answer::json(
                    if uncertain {
                        reqwest::StatusCode::BAD_GATEWAY
                    } else {
                        reqwest::StatusCode::OK
                    },
                    json!({"ok":true,"result":result(&content,-42,Some(7))}),
                ));
                let provider = super::adapter_test_server::HttpServer::start().await;
                let keys = Arc::new(fixture::keys());
                let relay = RelayClient::new("https://nyx.example", Duration::from_secs(5))
                    .unwrap()
                    .with_telegram(f.api.clone())
                    .with_native_telegram_structured_replies(true)
                    .with_interactions(Arc::new(InteractionStore::new(
                        s.server.records.clone(),
                        keys.clone(),
                        provider.client(true, Duration::from_secs(2)),
                    )));
                let disabled_relay = relay.clone().with_native_telegram_structured_replies(false);
                let (delivery, rx) = lane(&s);
                let engine = DeliveryEngine::start(
                    rx,
                    keys,
                    relay,
                    DeliveryConfig::default(),
                    TurnTracker::new(Readiness::initialized()),
                    CustomDelivery::new("https://gateway.example".into(), Arc::default()),
                );
                let message: OutboundMessage =
                    serde_json::from_value(json!({"structured_content":content})).unwrap();
                for (delayed, key) in [(false, None), (true, Some("delayed-facts"))] {
                    assert!(delivery
                        .tool_replies()
                        .reply_with_idempotency(
                            AgentCaller { key_hash: [9; 32] },
                            &channel,
                            &event,
                            message.clone(),
                            delayed,
                            key,
                        )
                        .await
                        .is_err());
                }
                assert!(f.calls().is_empty());
                let first = delivery
                    .tool_replies()
                    .reply_with_idempotency(
                        AgentCaller { key_hash: [9; 32] },
                        &channel,
                        &event,
                        message.clone(),
                        false,
                        Some("static-facts-one"),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    first.delivery,
                    if uncertain {
                        DeliveryOutcome::Unknown {
                            reason: "telegram_response_unconfirmed".into(),
                        }
                    } else {
                        DeliveryOutcome::Confirmed {
                            platform_message_id: Some("800".into()),
                        }
                    }
                );
                assert!(first.message_ref.is_none());
                assert!(first.message_parts.is_empty());
                assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
                s.journal().release(&mut lease).await.unwrap();
                let _resumed = accepted(&s).await;
                let (delivery, rx) = lane(&s);
                let engine = DeliveryEngine::start(
                    rx,
                    Arc::new(fixture::keys()),
                    disabled_relay,
                    DeliveryConfig::default(),
                    TurnTracker::new(Readiness::initialized()),
                    CustomDelivery::new("https://gateway.example".into(), Arc::default()),
                );
                let replay = delivery
                    .tool_replies()
                    .reply_with_idempotency(
                        AgentCaller { key_hash: [9; 32] },
                        &channel,
                        &event,
                        message.clone(),
                        false,
                        Some("static-facts-one"),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    replay.delivery,
                    if uncertain {
                        DeliveryOutcome::Unknown {
                            reason: "routed_reply_uncertain".into(),
                        }
                    } else {
                        first.delivery.clone()
                    }
                );
                assert_eq!(f.calls().len(), 1);
                let body: serde_json::Value = serde_json::from_slice(&f.calls()[0].body).unwrap();
                assert_eq!(body["chat_id"], "-42");
                assert_eq!(body["message_thread_id"], "7");
                let changed = serde_json::from_value(json!({
                    "structured_content": {"type":"location","latitude":2.0,"longitude":3.0}
                }))
                .unwrap();
                assert!(delivery
                    .tool_replies()
                    .reply_with_idempotency(
                        AgentCaller { key_hash: [9; 32] },
                        &channel,
                        &event,
                        changed,
                        false,
                        Some("static-facts-one")
                    )
                    .await
                    .is_err());
                assert_eq!(f.calls().len(), 1);
                {
                    let mut current = s.current.channel.lock().unwrap();
                    Arc::make_mut(&mut current).definition.sources.clear();
                }
                assert!(delivery
                    .tool_replies()
                    .reply_with_idempotency(
                        AgentCaller { key_hash: [9; 32] },
                        &channel,
                        &event,
                        message,
                        false,
                        Some("static-facts-two")
                    )
                    .await
                    .is_err());
                assert_eq!(f.calls().len(), 1);
                assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
            }
        }
    }
}
