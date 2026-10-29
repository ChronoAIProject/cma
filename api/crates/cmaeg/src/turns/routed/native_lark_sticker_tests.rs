//! Received sticker replies retain the ordinary target's durable replay/unknown fence.
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
    ingress::routing_store::test_server::Server,
    interactions::InteractionStore,
    native_lark::{
        self, message,
        outbound::tests::fixture::{confirmed, Api},
        receipts::Store,
        test_support as native,
    },
    net::exchange::TransportError,
    turns::TurnTracker,
    DeliveryConfig,
};
use cmaeg_core::{channel::Source, message::OutboundMessage};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

async fn setup(named: bool) -> Setup {
    let request = fixture::request(
        |v| {
            v["sources"] = json!([native::source()]);
            let c = native::credentials();
            v["secrets"]["native_lark"] = json!({"app_secret":c.app_secret.as_str(),"encrypt_key":c.encrypt_key.as_str(),"verification_token":c.verification_token.as_str()});
            v["routing"]["default_targets"] =
                json!([if named { fixture::REVIEWER } else { "default" }]);
            v["reply"]["sinks"] = json!([{"type":"source"}]);
        },
        fixture::message("unused"),
    );
    let Source::NativeLark(source) = native::source() else {
        panic!()
    };
    let mut raw = native::text("om_sticker", "", fixture::NOW);
    raw["event"]["message"]["message_type"] = "sticker".into();
    raw["event"]["message"]["content"] = json!({"file_key":"retained-key"}).to_string().into();
    let message = message::extract(&source, &raw, fixture::NOW)
        .unwrap()
        .unwrap();
    let launch = message::launch(
        request.channel,
        &source,
        &message,
        None,
        &fixture::keys(),
        Instant::now(),
        fixture::NOW,
    )
    .unwrap();
    let s = Setup::with_source(supported(), launch, &serde_json::to_vec(&message).unwrap()).await;
    {
        let mut channel = s.current.channel.lock().unwrap();
        let channel = Arc::make_mut(&mut channel);
        channel.agent_key_hash = [9; 32];
        channel.consumer_keys = vec![(s.target().launch.consumer.clone(), [9; 32])];
    }
    s
}
fn request() -> OutboundMessage {
    serde_json::from_value(json!({"reply_with_received_sticker":true})).unwrap()
}

#[tokio::test]
async fn received_sticker_default_named_confirmed_unknown_replays_never_send_twice() {
    for named in [false, true] {
        for outcome in ["confirmed", "unknown", "revoked"] {
            let unknown = outcome == "unknown";
            let s = setup(named).await;
            let mut lease = accepted(&s).await;
            let event = event(&s.journal().body(&lease).await.unwrap());
            let channel = s.current.channel.lock().unwrap().clone();
            let api = Api::new(vec![if unknown {
                Err(TransportError::Unavailable)
            } else {
                confirmed("om_sticker_sent", false)
            }]);
            let transport: Arc<dyn native_lark::Transport> = if outcome == "revoked" {
                Arc::new(RevokeBeforeSend {
                    inner: api.clone(),
                    current: s.current.clone(),
                })
            } else {
                api.clone()
            };
            let provider = super::adapter_test_server::HttpServer::start().await;
            let keys = Arc::new(fixture::keys());
            // Native delivery receipts use wall time; the routed journal keeps
            // its deterministic admission clock. Match the receipt store's TTL
            // clock to the real sender, as in the native media delivery tests.
            let receipt_records = Server::start_realtime().await;
            let receipts = Arc::new(Store::new(receipt_records.records.clone(), keys.clone()));
            let relay = RelayClient::new("https://nyx.example", Duration::from_secs(5))
                .unwrap()
                .with_native_lark_sticker_replies(true)
                .with_native_lark(
                    native_lark::Client::production()
                        .unwrap()
                        .with_transport(transport),
                    receipts.clone(),
                )
                .with_interactions(Arc::new(InteractionStore::new(
                    s.server.records.clone(),
                    keys.clone(),
                    provider.client(true, Duration::from_secs(2)),
                )));
            let (delivery, rx) = lane(&s);
            let engine = DeliveryEngine::start(
                rx,
                keys,
                relay,
                DeliveryConfig::default(),
                TurnTracker::new(Readiness::initialized()),
                CustomDelivery::new("https://gateway.example".into(), Arc::default()),
            );
            let first = delivery
                .tool_replies()
                .reply(
                    AgentCaller { key_hash: [9; 32] },
                    &channel,
                    &event,
                    request(),
                    false,
                )
                .await
                .unwrap()
                .unwrap();
            if outcome == "revoked" {
                assert_eq!(first.delivery.kind(), "rejected");
                assert!(api.writes.lock().unwrap().is_empty());
                assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
                continue;
            }
            assert_eq!(
                first.delivery.kind(),
                if unknown { "unknown" } else { "confirmed" },
                "{named} {outcome}: {:?}",
                first.delivery
            );
            let stored = receipts
                .load(
                    &event.channel_id,
                    &s.plan.activity.source,
                    "om_sticker_sent",
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                )
                .await
                .unwrap();
            if unknown {
                assert!(stored.is_none());
            } else {
                let stored = stored.expect("confirmed native sticker receipt");
                assert_eq!(stored.original, s.plan.identity);
                assert_eq!(stored.consumer, s.target().launch.consumer);
                assert_eq!(stored.provider, s.target().launch.provider);
            }
            s.journal().release(&mut lease).await.unwrap();
            let _resumed = accepted(&s).await;
            let replay = delivery
                .tool_replies()
                .reply(
                    AgentCaller { key_hash: [9; 32] },
                    &channel,
                    &event,
                    request(),
                    false,
                )
                .await
                .unwrap()
                .unwrap();
            if unknown {
                assert_eq!(
                    first.delivery,
                    DeliveryOutcome::Unknown {
                        reason: "native_lark_acceptance_unconfirmed".into()
                    }
                );
                assert_eq!(
                    replay.delivery,
                    DeliveryOutcome::Unknown {
                        reason: "routed_reply_uncertain".into()
                    }
                );
            } else {
                assert_eq!(replay.delivery, first.delivery);
            }
            assert_eq!(api.writes.lock().unwrap().len(), 1);
            // A removed consumer loses authority even though the result is durable.
            {
                let mut current = s.current.channel.lock().unwrap();
                let current = Arc::make_mut(&mut current);
                if named {
                    current
                        .definition
                        .routing
                        .as_mut()
                        .unwrap()
                        .consumers
                        .clear();
                } else {
                    current.definition.routing = None;
                }
            }
            let denied = delivery
                .tool_replies()
                .reply(
                    AgentCaller { key_hash: [9; 32] },
                    &channel,
                    &event,
                    request(),
                    false,
                )
                .await;
            assert!(
                denied.is_err()
                    || denied.is_ok_and(|r| r.is_none_or(|r| r.delivery.kind() == "rejected"))
            );
            assert_eq!(api.writes.lock().unwrap().len(), 1);
            assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
        }
    }
}

/// Mutation after credential/custody verification must be caught by the per-effect gate.
struct RevokeBeforeSend {
    inner: Arc<Api>,
    current: Arc<Current>,
}
impl native_lark::Transport for RevokeBeforeSend {
    fn exchange(
        &self,
        method: reqwest::Method,
        url: reqwest::Url,
        headers: reqwest::header::HeaderMap,
        body: Vec<u8>,
    ) -> futures_util::future::BoxFuture<'_, Result<crate::net::exchange::HttpReply, TransportError>>
    {
        let revoke = url.path().ends_with("/bot/v3/info");
        let result = self.inner.exchange(method, url, headers, body);
        if revoke {
            let mut channel = self.current.channel.lock().unwrap();
            Arc::make_mut(&mut channel).definition.routing = None;
        }
        result
    }
}
