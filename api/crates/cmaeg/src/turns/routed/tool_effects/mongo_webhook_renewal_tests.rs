use super::{super::replica_test_fixture::Harness, *};
use crate::{
    delivery::{
        custom::CustomDelivery,
        durable::DeliveryCoordinator,
        router::ShardedScheduler,
        sync::SyncRegistry,
        webhook_client::{TransportError, WebhookClient, WebhookTransport},
    },
    ingress::routing_plan::fixture,
    turns::routed::{test_support::recovery_frame, ReplayEffect, RoutedDelivery},
};
use futures_util::{future::BoxFuture, FutureExt};
use std::{
    net::SocketAddr,
    panic::AssertUnwindSafe,
    sync::atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

pub(in crate::turns::routed) struct HangingWebhook {
    pub started: Notify,
    pub calls: AtomicUsize,
}
impl Default for HangingWebhook {
    fn default() -> Self {
        Self {
            started: Notify::new(),
            calls: AtomicUsize::new(0),
        }
    }
}
impl WebhookTransport for HangingWebhook {
    fn resolve(&self, _: reqwest::Url) -> BoxFuture<'_, Result<Vec<SocketAddr>, TransportError>> {
        Box::pin(async { Ok(vec!["8.8.8.8:443".parse().unwrap()]) })
    }
    fn post(
        &self,
        _: reqwest::Url,
        _: Vec<SocketAddr>,
        _: reqwest::header::HeaderMap,
        _: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, TransportError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        Box::pin(std::future::pending())
    }
}
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn custom_webhook_renewal_loss_does_not_retry_or_commit() {
    let request = fixture::request(
        |v| {
            v["routing"]["default_targets"] = serde_json::json!(["default"]);
            v["sources"] =
                serde_json::json!([{"type":"custom","auth":{"type":"hmac","key_id":"result"}}]);
            v["partition"] = serde_json::json!({"version":1,"key":"per_event"});
            v["reply"]["sinks"] = serde_json::json!([{"type":"result_webhook","url":"https://receiver.example/hook","hmac_key_id":"result"}]);
            v["secrets"]["hmac_secrets"] =
                serde_json::json!([["result", "fixture-webhook-signing"]]);
        },
        fixture::custom(None),
    );
    let h = Harness::with_request(request, false).await;
    let result = AssertUnwindSafe(async {
        let (owner, mut lease) = h.accepted().await;
        let receiver = Arc::new(HangingWebhook::default());
        let mut custom = CustomDelivery::new(
            "https://gateway.example".into(),
            Arc::new(SyncRegistry::default()),
        );
        custom.webhooks = WebhookClient::with_transport(receiver.clone());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let delivery = RoutedDelivery::new(
            h.nodes[owner].plans(),
            h.nodes[owner].journal(),
            h.current.clone(),
            DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx)))
                .with_custom_stream(custom),
        );
        let frame = recovery_frame("fixture-response", 0, true);
        let (result, ()) = tokio::join!(
            delivery.deliver_frame(
                &h.plan.identity,
                &h.target().id,
                &mut lease,
                &frame,
                ReplayEffect::NoEffect
            ),
            async {
                receiver.started.notified().await;
                h.nodes[owner].lose_work(&h.target().id).await.unwrap();
            }
        );
        assert!(matches!(result, Err(ExecutionError::LostLease)));
        assert_eq!(receiver.calls.load(Ordering::SeqCst), 1);
    })
    .catch_unwind()
    .await;
    h.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
