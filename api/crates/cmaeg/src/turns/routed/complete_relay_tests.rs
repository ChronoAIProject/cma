//! Complete NyxID replies through the real engine and sealed delivery journal.
//! The transport fixture proves CMAEG's boundary, not NyxID/platform chunking.
use super::*;
use crate::delivery::{
    custom::CustomDelivery,
    engine::DeliveryEngine,
    relay_client::{RelayClient, RelayTransport},
    render::{LiteralText, Renderer},
};
use crate::net::exchange::{HttpReply, TransportError};
use cmaeg_core::{activity::SourceKind, channel::Platform};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, StatusCode, Url};
use serde_json::json;
use std::{sync::Mutex, time::Duration};
use tokio::sync::Notify;

struct RelayRequest {
    url: Url,
    headers: HeaderMap,
    body: Vec<u8>,
    timeout: Duration,
}

struct Relay {
    started: Notify,
    finish: Notify,
    requests: Mutex<Vec<RelayRequest>>,
    outcome: &'static str,
}
impl RelayTransport for Relay {
    fn post(
        &self,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        timeout: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.requests.lock().unwrap().push(RelayRequest {
            url,
            headers,
            body,
            timeout,
        });
        self.started.notify_one();
        Box::pin(async move {
            self.finish.notified().await;
            match self.outcome {
                "lost_response" => Err(TransportError::Unavailable),
                "partial" => Ok(HttpReply::new(StatusCode::BAD_GATEWAY, Vec::new())),
                "rate_limited" => Ok(HttpReply::new(StatusCode::TOO_MANY_REQUESTS, Vec::new())),
                _ => Ok(HttpReply::new(
                    StatusCode::OK,
                    br#"{"message_id":"outbound-receipt","platform_message_id":"last-part"}"#
                        .to_vec(),
                )),
            }
        })
    }
}

fn engine(setup: &Setup, relay: Arc<Relay>) -> (RoutedDelivery, DeliveryEngine) {
    let (tx, rx) = mpsc::channel(4);
    let engine = DeliveryEngine::start(
        rx,
        Arc::new(fixture::keys()),
        RelayClient::new("https://nyx.example", Duration::from_secs(5))
            .unwrap()
            .with_complete_replies(true)
            .with_transport(relay),
        crate::DeliveryConfig::default(),
        crate::turns::TurnTracker::new(Default::default()),
        CustomDelivery::new("https://gateway.example".into(), Arc::default()),
    );
    (
        RoutedDelivery::new(
            setup.server.store(),
            setup.journal().clone(),
            setup.current.clone(),
            DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx))),
        ),
        engine,
    )
}

#[tokio::test]
async fn complete_text_receipt_and_worker_loss_survive_engine_recreation_without_replay() {
    for outcome in [
        "confirmed",
        "partial",
        "lost_response",
        "rate_limited",
        "worker_loss",
    ] {
        let mut activity = fixture::message("long reply requested");
        activity.source = SourceKind::NyxidRelay {
            platform: Platform::Lark,
            route_id: fixture::ROUTE.into(),
        };
        let setup = Setup::with_request(supported(), fixture::request(|_| {}, activity)).await;
        let relay = Arc::new(Relay {
            started: Notify::new(),
            finish: Notify::new(),
            requests: Mutex::default(),
            outcome,
        });
        let mut lease = accepted(&setup).await;
        let (delivery, first_engine) = engine(&setup, relay.clone());
        let text = format!(
            "{}\nhttps://example.com/{}\n```rust\nlet x = [1, 2];\n```\nend-of-reply",
            "中文🦀".repeat(1400),
            "segment/".repeat(800)
        );
        let message: OutboundMessage = serde_json::from_value(json!({"text":text})).unwrap();
        message.validate().unwrap();
        let mut capabilities = crate::tools::Capabilities::relay(Platform::Lark);
        capabilities.buttons = "numbered_text".into();
        let (parts, _) = LiteralText.render(&message, &capabilities);
        assert!(parts.len() > 1);
        let expected = parts.into_iter().map(|part| part.text).collect::<String>();
        let effect = ReplayEffect::Reply(Box::new(message));
        let frame = recovery_frame("resp_1", 0, false);
        let mut pending = Box::pin(delivery.deliver_frame(
            &setup.plan.identity,
            &setup.target().id,
            &mut lease,
            &frame,
            effect.clone(),
        ));
        tokio::select! {
            _ = relay.started.notified() => {},
            result = &mut pending => panic!("delivery settled before HTTP: {outcome}: {}", result.is_ok()),
        }
        let checkpoint = stored(&setup).await;
        assert_eq!(cursor(&checkpoint), None);
        let checkpoint = checkpoint.delivery.unwrap();
        assert!(matches!(checkpoint.status, DeliveryStatus::Intent));
        assert_eq!(
            checkpoint.effect_hash,
            storage::hash(&storage::bytes(&effect).unwrap())
        );
        {
            let requests = relay.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            let request = &requests[0];
            assert_eq!(request.url.path(), "/api/v1/channel-relay/reply");
            assert_eq!(
                request.headers["authorization"],
                "Bearer fixture-channel-key"
            );
            assert_eq!(request.timeout, Duration::from_secs(15));
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["message_id"], "incoming-fixture");
            let sent = body["reply"]["text"].as_str().unwrap();
            assert_eq!(sent, expected);
            assert!(sent.ends_with("end-of-reply"));
        }
        if outcome == "worker_loss" {
            // Lose the worker after dispatch but before the platform receipt.
            // The engine may finish; no receipt reaches the abandoned worker.
            drop(pending);
            relay.finish.notify_one();
        } else {
            relay.finish.notify_one();
            let result = pending.await.unwrap();
            assert_eq!(
                matches!(result, FrameDelivery::Settled { .. }),
                outcome == "confirmed"
            );
        }
        drop(delivery);
        assert!(
            first_engine
                .drain(std::time::Instant::now() + Duration::from_secs(2))
                .await
        );
        setup.journal().release(&mut lease).await.unwrap();

        // Discard the engine's ephemeral token and event caches, then recover
        // solely from the retained journal using a new lease and coordinator.
        let (recovered, second_engine) = engine(&setup, relay.clone());
        let mut resumed = accepted(&setup).await;
        let result = recovered
            .deliver_frame(
                &setup.plan.identity,
                &setup.target().id,
                &mut resumed,
                &frame,
                effect.clone(),
            )
            .await
            .unwrap();
        if outcome == "confirmed" {
            let FrameDelivery::Settled {
                receipt: Some(receipt),
                cursor: 0,
            } = result
            else {
                panic!("lost confirmed receipt");
            };
            assert_eq!(
                receipt.delivery,
                DeliveryOutcome::Confirmed {
                    platform_message_id: Some("last-part".into())
                }
            );
            assert_eq!(cursor(&stored(&setup).await), Some(0));
        } else {
            assert!(matches!(result, FrameDelivery::Unknown));
            assert_eq!(cursor(&stored(&setup).await), None);
        }
        let ReplayEffect::Reply(mut changed) = effect else {
            unreachable!()
        };
        changed.text.as_mut().unwrap().push('!');
        assert!(matches!(
            recovered
                .deliver_frame(
                    &setup.plan.identity,
                    &setup.target().id,
                    &mut resumed,
                    &frame,
                    ReplayEffect::Reply(changed)
                )
                .await,
            Err(ExecutionError::Invalid)
        ));
        assert_eq!(
            relay.requests.lock().unwrap().len(),
            1,
            "{outcome} replayed"
        );
        drop(recovered);
        assert!(
            second_engine
                .drain(std::time::Instant::now() + Duration::from_secs(2))
                .await
        );
    }
}
