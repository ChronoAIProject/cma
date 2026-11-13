use super::{
    delivery_tests::{accepted, cursor, stored},
    projection_store_tests::{delta, frame},
    replay_projection::ReplayProjection,
    test_support::*,
    *,
};
use crate::delivery::{
    custom::CustomDelivery,
    durable::DeliveryCoordinator,
    intents::{DeliveryCommand, DeliveryOutcome},
    local_stream::{CustomStreamEffect, LocalStreamOutcome},
    router::ShardedScheduler,
    sync::{SyncRegistry, Waiter},
    webhook_client::{TransportError, WebhookClient, WebhookTransport},
};
use axum::response::{IntoResponse, Sse};
use cmaeg_core::seal::ReplyAddress;
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Url};
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    sync::{atomic::Ordering, Arc, Mutex},
};
use tokio::sync::mpsc;

#[path = "custom_delivery_tests/retained.rs"]
mod retained;

#[derive(Default)]
struct Receiver {
    events: Mutex<Vec<Value>>,
    statuses: Mutex<Vec<u16>>,
    on_post: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

#[test]
fn preadmission_requires_explicit_custom_stream_support() {
    let mut request = fixture::request(
        |value| {
            fixture::targets(value);
            value["reply"]["sinks"] = json!([{"type":"sync"}]);
        },
        fixture::custom(None),
    );
    fixture::rewrite_ref(&mut request, |event| {
        event.reply = ReplyAddress::Sync {
            waiter: uuid::Uuid::new_v4().to_string(),
        }
    });
    let plan = fixture::routed(&request);
    assert_eq!(
        validate_routed_delivery(&request.channel, &plan),
        Err(ExecutionError::Unsupported)
    );
    assert_eq!(
        validate_routed_delivery_with_sync(&request.channel, &plan, true),
        Ok(())
    );
}
impl WebhookTransport for Receiver {
    fn resolve(&self, _url: Url) -> BoxFuture<'_, Result<Vec<SocketAddr>, TransportError>> {
        Box::pin(async { Ok(vec!["8.8.8.8:443".parse().unwrap()]) })
    }
    fn post(
        &self,
        _url: Url,
        _addresses: Vec<SocketAddr>,
        headers: HeaderMap,
        body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>> {
        assert!(headers.contains_key("webhook-signature"));
        self.events
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&body).unwrap());
        if let Some(callback) = self.on_post.lock().unwrap().take() {
            callback();
        }
        let mut statuses = self.statuses.lock().unwrap();
        let status = if statuses.is_empty() {
            200
        } else {
            statuses.remove(0)
        };
        Box::pin(async move { Ok(status) })
    }
}
async fn setup(waiter: &str, webhooks: usize, auto_final: bool) -> Setup {
    let mut request = fixture::request(
        |value| {
            fixture::targets(value);
            value["reply"]["sinks"] = json!([{"type":"sync"}]);
            for index in 0..webhooks {
                value["reply"]["sinks"].as_array_mut().unwrap().push(json!({
                "type":"result_webhook", "url":format!("https://receiver.example/{index}"), "hmac_key_id":"result"
            }));
            }
            value["secrets"]["hmac_secrets"] = json!([["result", "fixture-webhook-signing"]]);
            for consumer in value["routing"]["consumers"].as_array_mut().unwrap() {
                consumer["reply"] = json!({"auto_final":auto_final,"destination":"origin"});
            }
        },
        fixture::custom(None),
    );
    fixture::rewrite_ref(&mut request, |event| {
        event.reply = ReplyAddress::Sync {
            waiter: waiter.into(),
        }
    });
    Setup::with_request(supported(), request).await
}
fn lane(
    setup: &Setup,
    waiters: Arc<SyncRegistry>,
    receiver: Arc<Receiver>,
) -> (RoutedDelivery, mpsc::Receiver<DeliveryCommand>) {
    let (tx, rx) = mpsc::channel(1);
    let mut custom = CustomDelivery::new("https://gateway.example".into(), waiters);
    custom.webhooks = WebhookClient::with_transport(receiver);
    (
        RoutedDelivery::new(
            setup.server.store(),
            setup.journal().clone(),
            setup.current.clone(),
            DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx)))
                .with_custom_stream(custom),
        ),
        rx,
    )
}
async fn stream(waiter: Waiter) -> String {
    let response = Sse::new(waiter.stream()).into_response();
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        axum::body::to_bytes(response.into_body(), 1024 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}
async fn deliver(
    setup: &Setup,
    delivery: &RoutedDelivery,
    lease: &mut Lease,
    frame: &crate::providers::RecoveryEvent,
    effect: ReplayEffect,
) -> FrameDelivery {
    delivery
        .deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            lease,
            frame,
            effect,
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn expired_executor_closes_only_original_primary_stream_without_content_or_external_sends() {
    use super::{
        adapter_test_server::HttpServer,
        scheduler::{Disposition, TargetExecutor, WorkItem},
    };
    use crate::{http::readiness::Readiness, turns::tracker::TurnTracker, DeliveryConfig};
    use std::time::Duration;

    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 1, true).await;
    let receiver = Arc::new(Receiver::default());
    let http = HttpServer::start().await;
    setup
        .server
        .memory
        .clock
        .store(fixture::NOW + 900, Ordering::SeqCst);
    setup.current.revoke_at.store(0, Ordering::SeqCst);
    let make_executor = || {
        let port = Arc::new(HttpExecutionPort::new(
            http.client(true, Duration::from_secs(60)),
            setup.port.clone(),
            setup.server.store(),
            setup.journal().clone(),
            setup.current.clone(),
        ));
        let (delivery, rx) = lane(&setup, waiters.clone(), receiver.clone());
        (
            RoutedTargetExecutor::new(
                port,
                delivery,
                TurnTracker::new(Readiness::initialized()),
                &DeliveryConfig::default(),
            ),
            rx,
        )
    };
    let (runner, mut rx) = make_executor();
    assert!(setup.plan.targets.len() > 1);
    let secondary = WorkItem {
        identity: setup.plan.identity.clone(),
        target_id: setup.plan.targets[1].id.clone(),
    };
    assert!(matches!(
        runner.execute(&secondary).await,
        Disposition::Settled
    ));
    assert_eq!(waiters.len(), 1);
    let primary = WorkItem {
        identity: setup.plan.identity.clone(),
        target_id: setup.target().id.clone(),
    };
    assert!(matches!(
        runner.execute(&primary).await,
        Disposition::Settled
    ));
    assert!(waiters.is_empty());
    let (restarted, mut restarted_rx) = make_executor();
    assert!(matches!(
        restarted.execute(&primary).await,
        Disposition::Settled
    ));
    let stream = stream(waiter).await;
    assert_eq!(stream.matches("event: cmaeg.failed").count(), 1);
    let data: Value = serde_json::from_str(
        stream
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(data["event_id"], json!(setup.plan.identity.event_id));
    assert_eq!(
        data["conversation_id"],
        json!(setup.target().launch.event.conversation_id)
    );
    assert!(data["text"].is_null());
    assert!(data["message"].is_null());
    assert_eq!(data["error"]["code"], "event_ref_expired");
    assert_eq!(data["error"]["status"], 410);
    assert_eq!(data["error"]["detail"], super::expiry::EXPIRED_BEFORE_START);
    assert!(http.seen().is_empty());
    assert!(receiver.events.lock().unwrap().is_empty());
    assert!(rx.try_recv().is_err());
    assert!(restarted_rx.try_recv().is_err());
    assert_eq!(setup.port.prepares.load(Ordering::SeqCst), 0);
    assert_eq!(setup.current.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn local_terminal_is_independent_of_auto_final_and_tool_suppression() {
    for (auto_final, tool_replied) in [(false, false), (true, true)] {
        let waiters = Arc::new(SyncRegistry::default());
        let (id, waiter) = waiters.register().unwrap();
        let setup = setup(&id, 1, auto_final).await;
        let mut lease = accepted(&setup).await;
        let body = setup.journal().body(&lease).await.unwrap();
        let receiver = Arc::new(Receiver::default());
        let (delivery, _rx) = lane(&setup, waiters.clone(), receiver.clone());
        if tool_replied {
            let store = super::tool_reply_store::ReplyStore(setup.journal().clone());
            let super::tool_reply_store::Admission::Claimed(claim) = store
                .admit(
                    &setup.plan,
                    setup.target(),
                    super::tool_reply_store::Kind::Tool,
                    "f".repeat(64),
                )
                .await
                .unwrap()
            else {
                panic!("claimed")
            };
            store
                .finish(
                    &claim,
                    &crate::delivery::durable::receipt_for(DeliveryOutcome::Confirmed {
                        platform_message_id: None,
                    }),
                )
                .await
                .unwrap();
        }
        let mut projection = ReplayProjection::new("resp_1".into());
        let frame = frame(
            json!({"type":"response.completed","sequence_number":0,"response":{"id":"resp_1","output":[{"type":"message","text":"suppressed answer"}]}}),
        );
        let effect = projection
            .project(&frame, &body, tool_replied, "failed")
            .unwrap();
        assert!(matches!(
            effect,
            ReplayEffect::Custom(CustomStreamEffect::Completed { final_text: None })
        ));
        assert!(matches!(
            deliver(&setup, &delivery, &mut lease, &frame, effect).await,
            FrameDelivery::Settled { receipt: None, .. }
        ));
        let checkpoint = lease.state.delivery.as_ref().unwrap();
        let evidence = checkpoint.custom.as_ref().unwrap();
        assert_eq!(evidence.local, Some(LocalStreamOutcome::Buffered));
        assert!(matches!(
            evidence.webhooks[0].status,
            WebhookStatus::Settled {
                outcome: DeliveryOutcome::Confirmed { .. }
            }
        ));
        let text = stream(waiter).await;
        assert!(text.contains("cmaeg.completed"));
        assert!(!text.contains("suppressed answer"));
        assert!(waiters.is_empty());
        let events = receiver.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["type"], "cmaeg.response.completed");
        assert!(events[0]["data"]["text"].is_null());
    }
}

#[tokio::test]
async fn deltas_and_committed_messages_reach_only_the_primary_waiter() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 0, true).await;
    let receiver = Arc::new(Receiver::default());
    let (delivery, _rx) = lane(&setup, waiters.clone(), receiver);
    let secondary = &setup.plan.targets[1];
    let DispatchOutcome::Replay { mut lease, .. } = setup
        .dispatcher
        .dispatch(&setup.plan.identity, &secondary.id, "secondary")
        .await
        .unwrap()
    else {
        panic!("accepted")
    };
    assert!(matches!(
        delivery
            .deliver_frame(
                &setup.plan.identity,
                &secondary.id,
                &mut lease,
                &recovery_frame("resp_1", 0, true),
                ReplayEffect::NoEffect
            )
            .await
            .unwrap(),
        FrameDelivery::Settled { .. }
    ));
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
        None
    );
    assert_eq!(
        waiters.len(),
        1,
        "secondary terminal must not close primary"
    );
    let mut lease = accepted(&setup).await;
    let body = setup.journal().body(&lease).await.unwrap();
    let mut projection = ReplayProjection::new("resp_1".into());
    for frame in [
        delta(0, "hello"),
        frame(
            json!({"type":"response.output_text.done","sequence_number":1,"item_id":"message","text":"hello"}),
        ),
        recovery_frame("resp_1", 2, true),
    ] {
        let effect = projection.project(&frame, &body, false, "failed").unwrap();
        assert!(matches!(
            deliver(&setup, &delivery, &mut lease, &frame, effect).await,
            FrameDelivery::Settled { receipt: None, .. }
        ));
    }
    let text = stream(waiter).await;
    assert_eq!(text.matches("event: cmaeg.text.delta").count(), 1);
    assert_eq!(text.matches("event: cmaeg.message").count(), 1);
    assert_eq!(text.matches("event: cmaeg.completed").count(), 1);
    assert_eq!(cursor(&lease.state), Some(2));
}

#[tokio::test]
async fn missing_waiter_keeps_each_webhook_receipt_and_replay_does_not_resend() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    drop(waiter);
    let setup = setup(&id, 2, true).await;
    let receiver = Arc::new(Receiver::default());
    *receiver.statuses.lock().unwrap() = vec![200, 403];
    let (delivery, _rx) = lane(&setup, waiters, receiver.clone());
    let mut lease = accepted(&setup).await;
    let frame = recovery_frame("resp_1", 0, true);
    deliver(
        &setup,
        &delivery,
        &mut lease,
        &frame,
        ReplayEffect::NoEffect,
    )
    .await;
    let evidence = lease
        .state
        .delivery
        .as_ref()
        .unwrap()
        .custom
        .as_ref()
        .unwrap();
    assert_eq!(evidence.local, Some(LocalStreamOutcome::Unavailable));
    assert!(matches!(
        evidence.webhooks[0].status,
        WebhookStatus::Settled {
            outcome: DeliveryOutcome::Confirmed { .. }
        }
    ));
    assert!(matches!(
        evidence.webhooks[1].status,
        WebhookStatus::Settled {
            outcome: DeliveryOutcome::Rejected { .. }
        }
    ));
    setup.journal().release(&mut lease).await.unwrap();
    let mut resumed = accepted(&setup).await;
    deliver(
        &setup,
        &delivery,
        &mut resumed,
        &frame,
        ReplayEffect::NoEffect,
    )
    .await;
    assert_eq!(receiver.events.lock().unwrap().len(), 2);
    assert_eq!(cursor(&resumed.state), Some(0));
}

#[tokio::test]
async fn abandoned_local_intent_is_lost_and_never_replays_the_original_frame() {
    for enqueued in [false, true] {
        let waiters = Arc::new(SyncRegistry::default());
        let (id, waiter) = waiters.register().unwrap();
        let setup = setup(&id, 0, true).await;
        let (delivery, _rx) = lane(&setup, waiters.clone(), Arc::default());
        let mut lease = accepted(&setup).await;
        let frame = recovery_frame("resp_1", 0, true);
        setup.server.fault_after(
            setup.target().id.clone(),
            !enqueued,
            true,
            usize::from(enqueued),
        );
        assert!(delivery
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut lease,
                &frame,
                ReplayEffect::NoEffect
            )
            .await
            .is_err());
        assert_eq!(
            stored(&setup).await.delivery.unwrap().custom.unwrap().local,
            Some(LocalStreamOutcome::Pending)
        );
        setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
        let mut resumed = accepted(&setup).await;
        deliver(
            &setup,
            &delivery,
            &mut resumed,
            &frame,
            ReplayEffect::NoEffect,
        )
        .await;
        assert_eq!(
            resumed
                .state
                .delivery
                .as_ref()
                .unwrap()
                .custom
                .as_ref()
                .unwrap()
                .local,
            Some(LocalStreamOutcome::Lost)
        );
        let text = stream(waiter).await;
        assert_eq!(
            text.matches("event: cmaeg.completed").count(),
            usize::from(enqueued)
        );
        if !enqueued {
            assert!(text.contains("sync_delivery_lost"));
        }
        assert_eq!(cursor(&resumed.state), Some(0));
    }
}

#[tokio::test]
async fn local_receipt_before_crash_is_preserved_without_claiming_reader_delivery() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 0, true).await;
    let (delivery, _rx) = lane(&setup, waiters, Arc::default());
    let mut lease = accepted(&setup).await;
    let frame = recovery_frame("resp_1", 0, true);
    setup
        .server
        .fault_after(setup.target().id.clone(), false, true, 2);
    assert!(delivery
        .deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            ReplayEffect::NoEffect
        )
        .await
        .is_err());
    drop(waiter); // A queue receipt cannot prove the client consumed its frame.
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let mut resumed = accepted(&setup).await;
    let (restarted, _rx) = lane(&setup, Arc::default(), Arc::default());
    deliver(
        &setup,
        &restarted,
        &mut resumed,
        &frame,
        ReplayEffect::NoEffect,
    )
    .await;
    assert_eq!(
        resumed
            .state
            .delivery
            .as_ref()
            .unwrap()
            .custom
            .as_ref()
            .unwrap()
            .local,
        Some(LocalStreamOutcome::Buffered)
    );
    assert!(matches!(
        resumed.state.delivery.as_ref().unwrap().status,
        DeliveryStatus::Settled { receipt: None }
    ));
}

#[tokio::test]
async fn abandoned_webhook_intent_is_unknown_without_resend_or_lost_local_receipt() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 1, true).await;
    let receiver = Arc::new(Receiver::default());
    let target = setup.target().id.clone();
    let memory = setup.server.memory.clone();
    *receiver.on_post.lock().unwrap() =
        Some(Box::new(move || {
            memory.fault.lock().unwrap().replace(
                crate::ingress::routing_store::test_server::Fault {
                    id: target,
                    store: false,
                    lose_read: true,
                    skip: 0,
                },
            );
        }));
    let (delivery, _rx) = lane(&setup, waiters, receiver.clone());
    let mut lease = accepted(&setup).await;
    let frame = recovery_frame("resp_1", 0, true);
    assert!(delivery
        .deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            ReplayEffect::NoEffect
        )
        .await
        .is_err());
    assert!(stream(waiter).await.contains("cmaeg.completed"));
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let mut resumed = accepted(&setup).await;
    assert!(matches!(
        deliver(
            &setup,
            &delivery,
            &mut resumed,
            &frame,
            ReplayEffect::NoEffect
        )
        .await,
        FrameDelivery::Unknown
    ));
    let evidence = resumed
        .state
        .delivery
        .as_ref()
        .unwrap()
        .custom
        .as_ref()
        .unwrap();
    assert_eq!(evidence.local, Some(LocalStreamOutcome::Buffered));
    assert!(matches!(
        evidence.webhooks[0].status,
        WebhookStatus::Unknown
    ));
    assert_eq!(receiver.events.lock().unwrap().len(), 1);
    assert_eq!(cursor(&resumed.state), None);
}

#[tokio::test]
async fn confirmed_webhook_survives_lost_final_settlement_without_repeating_either_leg() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 1, true).await;
    let receiver = Arc::new(Receiver::default());
    let target = setup.target().id.clone();
    let memory = setup.server.memory.clone();
    *receiver.on_post.lock().unwrap() =
        Some(Box::new(move || {
            memory.fault.lock().unwrap().replace(
                crate::ingress::routing_store::test_server::Fault {
                    id: target,
                    store: false,
                    lose_read: true,
                    skip: 1,
                },
            );
        }));
    let (delivery, _rx) = lane(&setup, waiters, receiver.clone());
    let mut lease = accepted(&setup).await;
    let frame = recovery_frame("resp_1", 0, true);
    assert!(delivery
        .deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            ReplayEffect::NoEffect
        )
        .await
        .is_err());
    setup.server.memory.clock.fetch_add(31, Ordering::SeqCst);
    let mut resumed = accepted(&setup).await;
    assert!(matches!(
        deliver(
            &setup,
            &delivery,
            &mut resumed,
            &frame,
            ReplayEffect::NoEffect
        )
        .await,
        FrameDelivery::Settled { receipt: None, .. }
    ));
    assert_eq!(
        stream(waiter)
            .await
            .matches("event: cmaeg.completed")
            .count(),
        1
    );
    assert_eq!(receiver.events.lock().unwrap().len(), 1);
    assert_eq!(cursor(&resumed.state), Some(0));
}

#[tokio::test]
async fn opt_in_is_required_and_backpressure_never_becomes_platform_confirmation() {
    let waiters = Arc::new(SyncRegistry::default());
    let (id, waiter) = waiters.register().unwrap();
    let setup = setup(&id, 0, true).await;
    let mut lease = accepted(&setup).await;
    let (disabled, _rx) = super::delivery_tests::lane(&setup);
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
        let ReplayEffect::Reply(message) = super::delivery_tests::effect() else {
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
                let reply = super::delivery_tests::command(&mut rx).await;
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
