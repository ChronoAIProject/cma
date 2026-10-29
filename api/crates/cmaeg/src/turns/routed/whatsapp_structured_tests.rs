//! Static replies preserve original private custody and durable keyed results.
use super::{
    delivery_tests::{accepted, lane},
    test_support::*,
    whatsapp_template_tests::setup_with,
};
use crate::{
    auth::AgentCaller,
    delivery::{
        custom::CustomDelivery, engine::DeliveryEngine, intents::DeliveryOutcome,
        relay_client::RelayClient,
    },
    http::readiness::Readiness,
    interactions::InteractionStore,
    turns::TurnTracker,
    whatsapp::{
        self,
        outbound::tests::{confirmed, Graph},
        structured_tests::samples,
    },
    DeliveryConfig,
};
use cmaeg_core::message::OutboundMessage;
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[tokio::test]
async fn private_structured_replies_replay_after_gate_off_restart_and_keep_receipt_custody() {
    for named in [false, true] {
        for (content, _) in samples() {
            for uncertain in [false, true] {
                let s = setup_with(named, false).await;
                let mut lease = accepted(&s).await;
                let event = event(&s.journal().body(&lease).await.unwrap());
                let channel = s.current.channel.lock().unwrap().clone();
                let graph = Graph::new(vec![if uncertain {
                    Err(crate::net::exchange::TransportError::Unavailable)
                } else {
                    confirmed("wamid.static")
                }]);
                let keys = Arc::new(fixture::keys());
                let receipt_server =
                    crate::ingress::routing_store::test_server::Server::start_realtime().await;
                let receipts = Arc::new(whatsapp::receipts::Store::new(
                    receipt_server.records.clone(),
                    keys.clone(),
                ));
                let provider = super::adapter_test_server::HttpServer::start().await;
                let relay = RelayClient::new("https://nyx.example", Duration::from_secs(5))
                    .unwrap()
                    .with_native_whatsapp_structured_replies(true)
                    .with_whatsapp(
                        whatsapp::Client::production()
                            .unwrap()
                            .with_transport(graph.clone()),
                        receipts.clone(),
                    )
                    .with_interactions(Arc::new(InteractionStore::new(
                        s.server.records.clone(),
                        keys.clone(),
                        provider.client(true, Duration::from_secs(2)),
                    )));
                let disabled = relay.clone().with_native_whatsapp_structured_replies(false);
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
                    serde_json::from_value(json!({"structured_content": content})).unwrap();
                for (delayed, key) in [(false, None), (true, Some("delayed-static"))] {
                    assert!(delivery
                        .tool_replies()
                        .reply_with_idempotency(
                            AgentCaller { key_hash: [9; 32] },
                            &channel,
                            &event,
                            message.clone(),
                            delayed,
                            key
                        )
                        .await
                        .is_err());
                }
                assert!(graph.calls.lock().unwrap().is_empty());
                let first = delivery
                    .tool_replies()
                    .reply_with_idempotency(
                        AgentCaller { key_hash: [9; 32] },
                        &channel,
                        &event,
                        message.clone(),
                        false,
                        Some("static-one"),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    first.delivery,
                    if uncertain {
                        DeliveryOutcome::Unknown {
                            reason: "native_whatsapp_acceptance_unconfirmed".into(),
                        }
                    } else {
                        DeliveryOutcome::Confirmed {
                            platform_message_id: Some("wamid.static".into()),
                        }
                    }
                );
                let receipt = receipts
                    .load(
                        &event.channel_id,
                        &s.plan.activity.source,
                        "447700900123",
                        "wamid.static",
                        time::OffsetDateTime::now_utc().unix_timestamp(),
                    )
                    .await
                    .unwrap();
                if uncertain {
                    assert!(receipt.is_none());
                } else {
                    let receipt = receipt.unwrap();
                    assert_eq!(receipt.original, s.plan.identity);
                    assert_eq!(receipt.consumer, s.target().launch.consumer);
                    assert_eq!(receipt.provider, s.target().launch.provider);
                }
                assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
                s.journal().release(&mut lease).await.unwrap();
                let _resumed = accepted(&s).await;
                let (delivery, rx) = lane(&s);
                let engine = DeliveryEngine::start(
                    rx,
                    Arc::new(fixture::keys()),
                    disabled,
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
                        Some("static-one"),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                // Explicit keys replay the complete retained receipt, including
                // the original uncertain outcome, without another platform call.
                assert_eq!(
                    super::storage::bytes(&replay).unwrap(),
                    super::storage::bytes(&first).unwrap()
                );
                assert_eq!(graph.calls.lock().unwrap().len(), 1);
                let changed = serde_json::from_value(
                    json!({"structured_content":{"type":"location","latitude":0,"longitude":0}}),
                )
                .unwrap();
                assert!(delivery
                    .tool_replies()
                    .reply_with_idempotency(
                        AgentCaller { key_hash: [9; 32] },
                        &channel,
                        &event,
                        changed,
                        false,
                        Some("static-one")
                    )
                    .await
                    .is_err());
                Arc::make_mut(&mut s.current.channel.lock().unwrap())
                    .definition
                    .sources
                    .clear();
                assert!(delivery
                    .tool_replies()
                    .reply_with_idempotency(
                        AgentCaller { key_hash: [9; 32] },
                        &channel,
                        &event,
                        message,
                        false,
                        Some("static-two")
                    )
                    .await
                    .is_err());
                assert_eq!(graph.calls.lock().unwrap().len(), 1);
                assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
            }
        }
    }
}

#[tokio::test]
async fn retained_proof_refuses_groups_and_revoked_private_sources() {
    for group in [false, true] {
        let s = setup_with(false, group).await;
        let lease = accepted(&s).await;
        let event = event(&s.journal().body(&lease).await.unwrap());
        let channel = s.current.channel.lock().unwrap().clone();
        let keys = Arc::new(fixture::keys());
        assert_eq!(
            whatsapp::structured::authorized(
                s.server.records.clone(),
                keys.clone(),
                &channel,
                &event
            )
            .await,
            !group
        );
        let mut revoked = channel.clone();
        Arc::make_mut(&mut revoked).definition.secrets.whatsapp = None;
        assert!(
            !whatsapp::structured::authorized(s.server.records.clone(), keys, &revoked, &event)
                .await
        );
    }
}
