//! Native media uses the durable tool lane for exact default/named authority.
use super::{
    delivery_tests::{accepted, lane},
    test_support::*,
};
use crate::{
    auth::AgentCaller,
    delivery::{
        custom::CustomDelivery,
        engine::DeliveryEngine,
        intents::{DeliveryCommand, DeliveryOutcome},
        relay_client::RelayClient,
    },
    http::readiness::Readiness,
    ingress::routing_store::test_server::Server,
    native_lark::{self, outbound::tests::fixture::*, receipts::Store, test_support as native},
    net::exchange::{HttpReply, TransportError},
    turns::TurnTracker,
    DeliveryConfig,
};
use cmaeg_core::{
    activity::SourceKind, channel::Secret, message::OutboundMessage, seal::ReplyAddress,
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, Url};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

async fn setup(named: bool) -> Setup {
    let mut activity = fixture::message("send file");
    activity.actor.id = "ou_human".into();
    activity.conversation.id = "oc_native_chat".into();
    activity.source = SourceKind::NativeLark(native::identity());
    let mut request = fixture::request(
        |value| {
            value["sources"] = json!([native::source()]);
            let c = native::credentials();
            value["secrets"]["native_lark"] = json!({"app_secret":c.app_secret.as_str(),"encrypt_key":c.encrypt_key.as_str(),"verification_token":c.verification_token.as_str()});
            value["routing"]["default_targets"] =
                json!([if named { fixture::REVIEWER } else { "default" }]);
            value["reply"]["sinks"] = json!([{"type":"source"}]);
        },
        activity,
    );
    fixture::rewrite_ref(&mut request, |event| {
        event.reply = ReplyAddress::NativeLark(reply())
    });
    let setup = Setup::with_request(supported(), request).await;
    {
        let mut current = setup.current.channel.lock().unwrap();
        let channel = Arc::make_mut(&mut current);
        channel.agent_key_hash = [9; 32];
        channel.consumer_keys = vec![(setup.target().launch.consumer.clone(), [9; 32])];
    }
    setup
}
fn message() -> OutboundMessage {
    OutboundMessage {
        text: None,
        blocks: vec![],
        reply_to: None,
        thread_reply: false,
        whatsapp_template: None,
        native_attachments: vec![attachment("application/pdf")],
    }
}
fn engine(
    rx: mpsc::Receiver<DeliveryCommand>,
    io: Arc<dyn native_lark::Transport>,
    store: Arc<Store>,
) -> DeliveryEngine {
    DeliveryEngine::start(
        rx,
        Arc::new(fixture::keys()),
        RelayClient::new("https://nyx.example", Duration::from_secs(5))
            .unwrap()
            .with_native_lark_outbound_media(true)
            .with_native_lark(
                native_lark::Client::production()
                    .unwrap()
                    .with_transport(io),
                store,
            ),
        DeliveryConfig::default(),
        TurnTracker::new(Readiness::initialized()),
        CustomDelivery::new("https://gateway.example".into(), Arc::default()),
    )
}
#[tokio::test]
async fn native_lark_media_default_named_confirmed_and_unknown_replays_never_resend() {
    for named in [false, true] {
        for failure in ["none", "upload", "send"] {
            let setup = setup(named).await;
            let mut lease = accepted(&setup).await;
            let event = event(&setup.journal().body(&lease).await.unwrap());
            let channel = setup.current.channel.lock().unwrap().clone();
            let records = Server::start_realtime().await;
            let store = Arc::new(Store::new(
                records.records.clone(),
                Arc::new(fixture::keys()),
            ));
            let mut responses = vec![];
            if failure != "upload" {
                responses.push(uploaded(false));
            }
            responses.push(if failure == "none" {
                confirmed("om_pdf", false)
            } else {
                Err(TransportError::Unavailable)
            });
            let io = Api::new(responses);
            let (delivery, rx) = lane(&setup);
            let engine = engine(rx, io.clone(), store.clone());
            let result = delivery
                .tool_replies()
                .reply(
                    AgentCaller { key_hash: [9; 32] },
                    &channel,
                    &event,
                    message(),
                    false,
                )
                .await
                .unwrap()
                .unwrap();
            assert!(if failure == "none" {
                matches!(result.delivery, DeliveryOutcome::Confirmed { .. })
            } else {
                matches!(result.delivery, DeliveryOutcome::Unknown { .. })
            });
            let stored = store
                .load(
                    &event.channel_id,
                    &setup.plan.activity.source,
                    "om_pdf",
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                )
                .await
                .unwrap();
            if failure == "none" {
                let stored = stored.unwrap();
                assert_eq!(stored.original, setup.plan.identity);
                assert_eq!(stored.consumer, setup.target().launch.consumer);
                assert_eq!(stored.provider, setup.target().launch.provider);
                assert_eq!(stored.partition_key, event.partition_key);
                assert_eq!(stored.conversation_id, event.conversation_id);
            } else {
                assert!(stored.is_none());
            }
            setup.journal().release(&mut lease).await.unwrap();
            let _resumed = accepted(&setup).await;
            let replay = delivery
                .tool_replies()
                .reply(
                    AgentCaller { key_hash: [9; 32] },
                    &channel,
                    &event,
                    message(),
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
            assert_eq!(
                io.writes.lock().unwrap().len(),
                if failure == "upload" { 1 } else { 2 }
            );
            assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
        }
    }
}
struct Revoking {
    io: Arc<Api>,
    current: Arc<Current>,
    named: bool,
    defect: &'static str,
    after_upload: bool,
}
impl native_lark::Transport for Revoking {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        let revoke = url.path()
            == if self.after_upload {
                "/open-apis/im/v1/files"
            } else {
                "/open-apis/bot/v3/info"
            };
        let response = self.io.exchange(method, url, headers, body);
        if revoke {
            let mut current = self.current.channel.lock().unwrap();
            let current = Arc::make_mut(&mut current);
            match self.defect {
                "source" => current.definition.sources.clear(),
                "app_secret" => {
                    current
                        .definition
                        .secrets
                        .native_lark
                        .as_mut()
                        .unwrap()
                        .app_secret = Secret::new("rotated-fixture-app-secret")
                }
                "encrypt_key" => {
                    current
                        .definition
                        .secrets
                        .native_lark
                        .as_mut()
                        .unwrap()
                        .encrypt_key = Secret::new("rotated-fixture-encrypt-key")
                }
                "verification" => {
                    current
                        .definition
                        .secrets
                        .native_lark
                        .as_mut()
                        .unwrap()
                        .verification_token = Secret::new("rotated-fixture-verification")
                }
                "provider" if self.named => {
                    let other = current.consumer(None).unwrap().provider.clone();
                    current
                        .definition
                        .routing
                        .as_mut()
                        .unwrap()
                        .consumers
                        .iter_mut()
                        .find(|c| c.id.as_str() == fixture::REVIEWER)
                        .unwrap()
                        .provider = other;
                }
                "provider" => crate::channels::definition::fixture::replace_default_binding(
                    &mut current.definition,
                    "changed-provider".parse().unwrap(),
                ),
                "consumer" if self.named => current
                    .definition
                    .routing
                    .as_mut()
                    .unwrap()
                    .consumers
                    .clear(),
                "consumer" => current.definition.routing = None,
                _ => panic!(),
            }
        }
        response
    }
}
#[tokio::test]
async fn native_lark_media_rechecks_current_source_credentials_and_default_named_target_after_custody_and_upload(
) {
    for named in [false, true] {
        for after_upload in [false, true] {
            for defect in [
                "source",
                "app_secret",
                "encrypt_key",
                "verification",
                "provider",
                "consumer",
            ] {
                let setup = setup(named).await;
                let lease = accepted(&setup).await;
                let event = event(&setup.journal().body(&lease).await.unwrap());
                let channel = setup.current.channel.lock().unwrap().clone();
                let records = Server::start_realtime().await;
                let store = Arc::new(Store::new(
                    records.records.clone(),
                    Arc::new(fixture::keys()),
                ));
                let io = Api::new(if after_upload {
                    vec![uploaded(false)]
                } else {
                    vec![]
                });
                let transport = Arc::new(Revoking {
                    io: io.clone(),
                    current: setup.current.clone(),
                    named,
                    defect,
                    after_upload,
                });
                let (delivery, rx) = lane(&setup);
                let engine = engine(rx, transport, store);
                let result = delivery
                    .tool_replies()
                    .reply(
                        AgentCaller { key_hash: [9; 32] },
                        &channel,
                        &event,
                        message(),
                        false,
                    )
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    matches!(result.delivery, DeliveryOutcome::Rejected { .. }),
                    "{named} {after_upload} {defect}"
                );
                assert_eq!(io.writes.lock().unwrap().len(), usize::from(after_upload));
                assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
            }
        }
    }
}
