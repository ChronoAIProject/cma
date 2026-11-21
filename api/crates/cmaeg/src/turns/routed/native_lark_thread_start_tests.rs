use super::{
    delivery_tests::{accepted, lane},
    test_support::*,
};
use crate::{
    auth::AgentCaller,
    delivery::{
        complete_tests::native_lark_thread_start_fixture::Api, custom::CustomDelivery,
        engine::DeliveryEngine, intents::DeliveryOutcome, relay_client::RelayClient,
    },
    interactions::InteractionStore,
    native_lark::{self, outbound::tests::fixture::reply, receipts::Store, test_support as native},
    turns::{routed::adapter_test_server::HttpServer, TurnTracker},
    DeliveryConfig,
};
use cmaeg_core::{
    activity::{ConversationKind, SourceKind},
    channel::Secret,
    message::OutboundMessage,
    seal::ReplyAddress,
};
use serde_json::json;
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

async fn setup(named: bool) -> Setup {
    let mut activity = fixture::message("start a thread");
    activity.actor.id = "ou_human".into();
    activity.conversation.id = "oc_native_chat".into();
    activity.conversation.kind = ConversationKind::Group;
    activity.source = SourceKind::NativeLark(native::identity());
    activity.event_id =
        native_lark::message::original_id(&native::identity(), "om_original").unwrap();
    let mut request = fixture::request(
        |v| {
            v["sources"] = json!([native::source()]);
            let c = native::credentials();
            v["secrets"]["native_lark"] = json!({"app_secret":c.app_secret.as_str(),"encrypt_key":c.encrypt_key.as_str(),"verification_token":c.verification_token.as_str()});
            v["routing"]["default_targets"] =
                json!([if named { fixture::REVIEWER } else { "default" }]);
            v["reply"]["sinks"] = json!([{"type":"source"}]);
        },
        activity,
    );
    fixture::rewrite_ref(&mut request, |e| {
        e.reply = ReplyAddress::NativeLark(reply())
    });
    let setup = Setup::with_request(supported(), request).await;
    {
        let mut c = setup.current.channel.lock().unwrap();
        let c = Arc::make_mut(&mut c);
        c.agent_key_hash = [9; 32];
        c.consumer_keys = vec![(setup.target().launch.consumer.clone(), [9; 32])];
    }
    setup
}

#[tokio::test]
async fn native_lark_thread_start_durable_confirmed_unknown_replay_and_revocation_never_resend() {
    for named in [false, true] {
        for failure in ["none", "unknown", "source", "credential", "consumer"] {
            let setup = setup(named).await;
            let mut lease = accepted(&setup).await;
            let frozen = setup.journal().body(&lease).await.unwrap();
            let event = event(&frozen);
            let channel = setup.current.channel.lock().unwrap().clone();
            let provider = HttpServer::start().await;
            provider.memory.lock().unwrap().config.event_context =
                Some(serde_json::to_value(&frozen.event_context).unwrap());
            let contexts = Arc::new(InteractionStore::new(
                setup.server.records.clone(),
                Arc::new(fixture::keys()),
                provider.client(true, Duration::from_secs(2)),
            ));
            let receipts =
                crate::ingress::routing_store::test_server::Server::start_realtime().await;
            let store = Arc::new(Store::new(
                receipts.records.clone(),
                Arc::new(fixture::keys()),
            ));
            let api = Api::new(&setup.plan.activity);
            api.fail_send.store(failure == "unknown", Ordering::SeqCst);
            if matches!(failure, "source" | "credential" | "consumer") {
                let current = setup.current.clone();
                *api.after_anchor.lock().unwrap() = Some(Box::new(move || {
                    let mut c = current.channel.lock().unwrap();
                    let c = Arc::make_mut(&mut c);
                    match failure {
                        "source" => c.definition.sources.clear(),
                        "credential" => {
                            c.definition
                                .secrets
                                .native_lark
                                .as_mut()
                                .unwrap()
                                .app_secret = Secret::new("rotated-native-app-secret")
                        }
                        _ if named => c.definition.routing.as_mut().unwrap().consumers.clear(),
                        _ => c.definition.routing = None,
                    }
                }));
            }
            let (delivery, rx) = lane(&setup);
            let start_engine = |rx| {
                DeliveryEngine::start(
                    rx,
                    Arc::new(fixture::keys()),
                    RelayClient::new("https://nyx.example", Duration::from_secs(5))
                        .unwrap()
                        .with_native_lark_thread_start(true)
                        .with_native_lark(
                            native_lark::Client::production()
                                .unwrap()
                                .with_transport(api.clone()),
                            store.clone(),
                        )
                        .with_interactions(contexts.clone()),
                    DeliveryConfig::default(),
                    TurnTracker::new(Default::default()),
                    CustomDelivery::new("https://gateway.example".into(), Arc::default()),
                )
            };
            let mut engine = start_engine(rx);
            let message: OutboundMessage =
                serde_json::from_value(json!({"text":"Continue here", "thread_reply":true}))
                    .unwrap();
            let result = delivery
                .tool_replies()
                .reply(
                    AgentCaller { key_hash: [9; 32] },
                    &channel,
                    &event,
                    message.clone(),
                    false,
                )
                .await;
            if matches!(failure, "none" | "unknown") {
                let result = result.unwrap().unwrap();
                assert_eq!(
                    result.delivery.kind(),
                    if failure == "none" {
                        "confirmed"
                    } else {
                        "unknown"
                    }
                );
                setup.journal().release(&mut lease).await.unwrap();
                assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
                drop(delivery);
                let (delivery, rx) = lane(&setup);
                engine = start_engine(rx);
                let _resumed = accepted(&setup).await;
                let replay = delivery
                    .tool_replies()
                    .reply(
                        AgentCaller { key_hash: [9; 32] },
                        &channel,
                        &event,
                        message,
                        false,
                    )
                    .await
                    .unwrap()
                    .unwrap();
                if failure == "none" {
                    assert_eq!(replay.delivery, result.delivery);
                } else {
                    assert!(matches!(replay.delivery, DeliveryOutcome::Unknown { .. }));
                }
                assert_eq!(api.writes(), 1);
                let stored = store
                    .load(
                        &event.channel_id,
                        &setup.plan.activity.source,
                        "om_thread_reply",
                        time::OffsetDateTime::now_utc().unix_timestamp(),
                    )
                    .await
                    .unwrap();
                assert_eq!(stored.is_some(), failure == "none");
                if let Some(stored) = stored {
                    assert_eq!(stored.reply.thread_id.as_deref(), Some("omt_started"));
                    assert_eq!(stored.original, setup.plan.identity);
                    assert_eq!(stored.consumer, setup.target().launch.consumer);
                }
            } else {
                assert!(
                    result.is_err()
                        || result
                            .unwrap()
                            .is_none_or(|r| matches!(r.delivery, DeliveryOutcome::Rejected { .. }))
                );
                assert_eq!(api.writes(), 0, "{failure}");
            }
            assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
        }
    }
}
