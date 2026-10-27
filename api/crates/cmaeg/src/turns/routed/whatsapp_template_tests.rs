//! Existing public tool replies compose native templates with durable ownership.
use super::{
    delivery_tests::{accepted, command, lane},
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
    turns::TurnTracker,
    whatsapp::{
        self,
        outbound::tests::{confirmed, Graph},
        templates::tests::rich,
    },
    DeliveryConfig,
};
use cmaeg_core::{
    activity::{ActorKind, SourceKind},
    seal::ReplyAddress,
};
use serde_json::json;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

async fn setup(named: bool) -> Setup {
    let mut activity = fixture::message("invoice please");
    activity.actor.id = "447700900123".into();
    activity.actor.kind = ActorKind::Unknown;
    activity.conversation.id = "447700900123".into();
    activity.source = SourceKind::Whatsapp {
        app_id: "12345".into(),
        waba_id: "23456".into(),
        phone_number_id: "34567".into(),
    };
    let mut request = fixture::request(
        |value| {
            value["sources"] = json!([whatsapp::tests::source()]);
            let credentials = whatsapp::tests::credentials();
            value["secrets"]["whatsapp"] = json!({"app_secret":credentials.app_secret.as_str(),"access_token":credentials.access_token.as_str(),"verify_token":credentials.verify_token.as_str()});
            value["routing"]["default_targets"] =
                json!([if named { fixture::REVIEWER } else { "default" }]);
            value["reply"]["sinks"] = json!([{"type":"source"}]);
        },
        activity,
    );
    fixture::rewrite_ref(&mut request, |event| {
        event.reply = ReplyAddress::Whatsapp {
            app_id: "12345".into(),
            waba_id: "23456".into(),
            phone_number_id: "34567".into(),
            recipient_id: "447700900123".into(),
            message_id: "wamid.original".into(),
        }
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
fn engine(
    rx: mpsc::Receiver<DeliveryCommand>,
    graph: Arc<Graph>,
    receipts: Arc<whatsapp::receipts::Store>,
) -> DeliveryEngine {
    DeliveryEngine::start(
        rx,
        Arc::new(fixture::keys()),
        RelayClient::new("https://nyx.example", Duration::from_secs(5))
            .unwrap()
            .with_native_whatsapp_templates(true)
            .with_whatsapp(
                whatsapp::Client::production()
                    .unwrap()
                    .with_transport(graph),
                receipts,
            ),
        DeliveryConfig::default(),
        TurnTracker::new(Readiness::initialized()),
        CustomDelivery::new("https://gateway.example".into(), Arc::default()),
    )
}

#[tokio::test]
async fn native_whatsapp_templates_retain_default_and_named_receipts_and_replay_without_resend() {
    for named in [false, true] {
        for uncertain in [false, true] {
            let setup = setup(named).await;
            let mut lease = accepted(&setup).await;
            let event = event(&setup.journal().body(&lease).await.unwrap());
            let channel = setup.current.channel.lock().unwrap().clone();
            let records = Server::start_realtime().await;
            let receipts = Arc::new(whatsapp::receipts::Store::new(
                records.records.clone(),
                Arc::new(fixture::keys()),
            ));
            let graph = Graph::new(vec![if uncertain {
                Err(crate::net::exchange::TransportError::Unavailable)
            } else {
                confirmed("wamid.template")
            }]);
            let (delivery, rx) = lane(&setup);
            let engine = engine(rx, graph.clone(), receipts.clone());
            let tools = delivery.tool_replies();
            let result = tools
                .reply(
                    AgentCaller { key_hash: [9; 32] },
                    &channel,
                    &event,
                    rich(),
                    false,
                )
                .await
                .unwrap()
                .unwrap();
            assert!(if uncertain {
                matches!(result.delivery, DeliveryOutcome::Unknown { .. })
            } else {
                matches!(result.delivery, DeliveryOutcome::Confirmed { .. })
            });
            assert!(result.message_ref.is_none());
            let stored = receipts
                .load(
                    &event.channel_id,
                    &setup.plan.activity.source,
                    "447700900123",
                    "wamid.template",
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                )
                .await
                .unwrap();
            if uncertain {
                assert!(stored.is_none());
            } else {
                let stored = stored.unwrap();
                assert_eq!(stored.original, setup.plan.identity);
                assert_eq!(stored.consumer, setup.target().launch.consumer);
                assert_eq!(stored.provider, setup.target().launch.provider);
                assert_eq!(stored.conversation_id, event.conversation_id);
                assert_eq!(stored.partition_key, event.partition_key);
            }
            setup.journal().release(&mut lease).await.unwrap();
            let _resumed = accepted(&setup).await;
            // A fresh tool facade uses the persisted result after resumption;
            // replay does not reopen a platform effect.
            let replay = delivery
                .tool_replies()
                .reply(
                    AgentCaller { key_hash: [9; 32] },
                    &channel,
                    &event,
                    rich(),
                    false,
                )
                .await
                .unwrap()
                .unwrap();
            if uncertain {
                assert_eq!(
                    replay.delivery,
                    DeliveryOutcome::Unknown {
                        reason: "routed_reply_uncertain".into()
                    }
                );
            } else {
                assert_eq!(replay.delivery, result.delivery);
            }
            assert_eq!(graph.calls.lock().unwrap().len(), 1);
            {
                let calls = graph.calls.lock().unwrap();
                let body: serde_json::Value = serde_json::from_slice(&calls[0].2).unwrap();
                assert_eq!(
                    body["template"],
                    serde_json::to_value(rich().whatsapp_template.unwrap()).unwrap()
                );
                assert_eq!(body["to"], "447700900123");
                assert_eq!(body["context"]["message_id"], "wamid.original");
                assert_eq!(calls[0].0.path(), "/v25.0/34567/messages");
            }
            assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
        }
    }
}

#[tokio::test]
async fn native_whatsapp_template_queued_sends_recheck_source_credential_and_consumer_authority() {
    for defect in 0..6 {
        let setup = setup(true).await;
        let lease = accepted(&setup).await;
        let event = event(&setup.journal().body(&lease).await.unwrap());
        let channel = setup.current.channel.lock().unwrap().clone();
        let records = Server::start_realtime().await;
        let receipts = Arc::new(whatsapp::receipts::Store::new(
            records.records.clone(),
            Arc::new(fixture::keys()),
        ));
        let graph = Graph::new(vec![]);
        let (delivery, mut rx) = lane(&setup);
        let tools = delivery.tool_replies();
        let (result, ()) = tokio::join!(
            tools.reply(
                AgentCaller { key_hash: [9; 32] },
                &channel,
                &event,
                rich(),
                false
            ),
            async {
                let reply = command(&mut rx).await;
                {
                    let mut current = setup.current.channel.lock().unwrap();
                    let current = Arc::make_mut(&mut current);
                    match defect {
                        0 => current.definition.sources.clear(),
                        1 => {
                            current.definition.secrets.whatsapp = None;
                            current.version += 1;
                        }
                        2 => current
                            .definition
                            .routing
                            .as_mut()
                            .unwrap()
                            .consumers
                            .clear(),
                        3 => current.consumer_keys.clear(),
                        4 => {
                            current
                                .definition
                                .routing
                                .as_mut()
                                .unwrap()
                                .consumers
                                .iter_mut()
                                .find(|consumer| consumer.id == setup.target().launch.consumer)
                                .unwrap()
                                .pending = true
                        }
                        5 => {
                            current
                                .definition
                                .routing
                                .as_mut()
                                .unwrap()
                                .consumers
                                .iter_mut()
                                .find(|consumer| consumer.id == setup.target().launch.consumer)
                                .unwrap()
                                .provider = current.definition.provider.clone()
                        }
                        _ => unreachable!(),
                    }
                }
                let (tx, receive) = mpsc::channel(1);
                let engine = engine(receive, graph.clone(), receipts);
                tx.send(DeliveryCommand::DurableReply(reply)).await.unwrap();
                drop(tx);
                assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
            }
        );
        assert!(
            matches!(
                result.unwrap().unwrap().delivery,
                DeliveryOutcome::Rejected { .. }
            ),
            "{defect}"
        );
        assert!(graph.calls.lock().unwrap().is_empty());
    }
}
