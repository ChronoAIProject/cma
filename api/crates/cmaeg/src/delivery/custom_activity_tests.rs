use super::{
    tests::{channel, event, message},
    *,
};
use crate::{
    activity::{Change, ConsumerState},
    delivery::gate::Ungated,
    turns::observer::ActivityObserver,
};
use cmaeg_core::ids::{EventId, RecordId};
use futures_util::future::BoxFuture;
use std::sync::Mutex;

#[derive(Default)]
struct Facts(Mutex<Vec<Change>>);
impl ActivityObserver for Facts {
    fn checkpoint<'a>(
        &'a self,
        _: &'a RecordId,
        _: &'a EventId,
        change: Change,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.0.lock().unwrap().push(change);
        })
    }
}
struct Mixed;
impl crate::delivery::webhook_client::WebhookTransport for Mixed {
    fn resolve(
        &self,
        _: reqwest::Url,
    ) -> BoxFuture<'_, Result<Vec<std::net::SocketAddr>, crate::net::exchange::TransportError>>
    {
        Box::pin(async { Ok(vec!["8.8.8.8:443".parse().unwrap()]) })
    }
    fn post(
        &self,
        url: reqwest::Url,
        _: Vec<std::net::SocketAddr>,
        _: reqwest::header::HeaderMap,
        _: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, crate::net::exchange::TransportError>> {
        Box::pin(async move {
            Ok(if url.host_str() == Some("accepted.example") {
                200
            } else {
                400
            })
        })
    }
}

#[tokio::test]
async fn legacy_custom_activity_keeps_mixed_webhook_receipts_and_local_buffer_distinct() {
    let channel = channel(|value| {
        value["reply"]["sinks"] = json!([
            {"type":"sync"},
            {"type":"result_webhook","url":"https://accepted.example/sensitive-path?token=private","hmac_key_id":"result"},
            {"type":"result_webhook","url":"https://rejected.example/result","hmac_key_id":"result"}
        ]);
        value["secrets"]["hmac_secrets"] = json!([["result", "fixture-signing-key"]]);
    });
    let waiters = Arc::new(SyncRegistry::default());
    let (waiter, _frames) = waiters.register().unwrap();
    let facts = Arc::new(Facts::default());
    let mut delivery =
        CustomDelivery::new("https://gateway.example".into(), waiters).with_activity(facts.clone());
    delivery.webhooks = WebhookClient::with_transport(Arc::new(Mixed));
    let event = event(&channel, ReplyAddress::Sync { waiter });
    let receipt = delivery.reply(&Ungated, &channel, &event, &message()).await;
    assert!(
        matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }),
        "compatibility aggregate stays unchanged"
    );
    let facts = facts.0.lock().unwrap();
    assert_eq!(facts.len(), 3);
    let deliveries: Vec<_> = facts
        .iter()
        .filter_map(|change| change.delivery.as_ref())
        .collect();
    assert_eq!(deliveries[0].outcome, "buffered");
    assert!(deliveries[0].confirmed_at.is_none());
    assert_eq!(deliveries[1].outcome, "confirmed");
    assert!(deliveries[1].confirmed_at.is_some());
    assert_eq!(deliveries[1].recipients[0].id, "https://accepted.example");
    assert_eq!(deliveries[2].outcome, "rejected");
    assert!(deliveries[2].confirmed_at.is_none());
    assert_eq!(deliveries[2].recipients[0].id, "https://rejected.example");
    assert!(!serde_json::to_string(&deliveries)
        .unwrap()
        .contains("sensitive-path"));
    let request = crate::ingress::routing_plan::fixture::request(
        |_| {},
        crate::ingress::routing_plan::fixture::message("fixture"),
    );
    let mut row = crate::activity::ChannelActivity {
        source_refusal: None,
        quoted_context: None,
        event_id: "event".into(),
        kind: None,
        event_type: None,
        source_occurred_at: None,
        source: request.activity.source.clone(),
        actor: request.activity.actor.clone(),
        conversation: request.activity.conversation.clone(),
        occurred_at: request.activity.occurred_at,
        received_at: crate::activity::now(),
        consumers: vec![],
    };
    for fact in facts.iter() {
        fact.apply(&mut row);
    }
    assert_eq!(
        row.consumers[0].deliveries.len(),
        3,
        "different recipients must not overwrite each other's receipt"
    );
    Change::state("default", ConsumerState::Completed, None).apply(&mut row);
}
