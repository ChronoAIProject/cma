//! Existing public tool replies compose native templates with durable ownership.
use super::whatsapp_template_tests::setup_with;
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
        groups::tests::{confirmed, proofs, template as rich, Graph, GROUP},
    },
    DeliveryConfig,
};
use cmaeg_core::channel::Source;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

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
            .with_native_whatsapp_groups(true)
            .with_native_whatsapp_outbound_media(true)
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
async fn native_whatsapp_groups_retain_default_and_named_receipts_and_replay_without_resend() {
    for named in [false, true] {
        for uncertain in [false, true] {
            let setup = setup_with(named, true).await;
            let mut lease = accepted(&setup).await;
            let event = event(&setup.journal().body(&lease).await.unwrap());
            let channel = setup.current.channel.lock().unwrap().clone();
            let records = Server::start_realtime().await;
            let receipts = Arc::new(whatsapp::receipts::Store::new(
                records.records.clone(),
                Arc::new(fixture::keys()),
            ));
            let mut replies = proofs();
            replies.push(if uncertain {
                Err(crate::net::exchange::TransportError::Unavailable)
            } else {
                confirmed("wamid.template")
            });
            let graph = Graph::new(replies);
            let (delivery, rx) = lane(&setup);
            let engine = engine(rx, graph.clone(), receipts.clone());
            let tools = delivery.tool_replies();
            let result = tools
                .reply(
                    AgentCaller { key_hash: [9; 32] },
                    &channel,
                    &event,
                    rich(),
                    uncertain,
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
                .load_group(
                    &event.channel_id,
                    &setup.plan.activity.source,
                    GROUP,
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
                assert_eq!(stored.group, Some(whatsapp::groups::tests::reply()));
                assert!(receipts
                    .load(
                        &event.channel_id,
                        &setup.plan.activity.source,
                        GROUP,
                        "wamid.template",
                        time::OffsetDateTime::now_utc().unix_timestamp()
                    )
                    .await
                    .unwrap()
                    .is_none());
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
                    uncertain,
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
            assert_eq!(graph.calls.lock().unwrap().len(), 4);
            {
                let calls = graph.calls.lock().unwrap();
                let body: serde_json::Value = serde_json::from_slice(&calls[3].2).unwrap();
                assert_eq!(
                    body["template"],
                    serde_json::to_value(rich().whatsapp_template.unwrap()).unwrap()
                );
                assert_eq!(body["to"], GROUP);
                assert_eq!(body["recipient_type"], "group");
                assert_eq!(body["context"]["message_id"], "wamid.original");
                assert_eq!(calls[3].1.path(), "/v25.0/34567/messages");
            }
            assert!(engine.drain(Instant::now() + Duration::from_secs(5)).await);
        }
    }
}

#[tokio::test]
async fn native_whatsapp_group_queued_sends_recheck_source_group_credential_and_consumer_authority()
{
    for defect in 0..7 {
        let setup = setup_with(true, true).await;
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
                    let replacement_provider = current.consumer(None).unwrap().provider.clone();
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
                                .provider = replacement_provider
                        }
                        6 => {
                            if let Source::Whatsapp { group_ids, .. } =
                                &mut current.definition.sources[0]
                            {
                                group_ids.clear();
                            }
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

#[tokio::test]
async fn native_whatsapp_group_retained_work_revokes_on_credential_rotation_without_version_change()
{
    for named in [false, true] {
        let setup = setup_with(named, true).await;
        let mut current = setup.current.channel.lock().unwrap().clone();
        let current = Arc::make_mut(&mut current);
        assert!(super::dispatcher::validate_current(current, &setup.plan, setup.target()).is_ok());
        current
            .definition
            .secrets
            .whatsapp
            .as_mut()
            .unwrap()
            .access_token = cmaeg_core::channel::Secret::new("rotated-access-token-fixture");
        assert!(super::dispatcher::validate_current(current, &setup.plan, setup.target()).is_err());
        assert!(
            whatsapp::credentials(&current.definition, &setup.target().launch.event.reply)
                .is_none()
        );
    }
}
