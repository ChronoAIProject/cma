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

mod local;
mod outcomes;
mod replay;
