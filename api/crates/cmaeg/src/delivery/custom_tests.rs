use super::*;
use crate::delivery::gate::recording::RecordingGate;
use cmaeg_core::{channel::ChannelDefinition, ids::ChannelId, seal::ChannelPointer};
use futures_util::{future::BoxFuture, StreamExt};

fn channel(edit: impl FnOnce(&mut Value)) -> ResolvedChannel {
    let mut value: Value = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    edit(&mut value);
    let definition: ChannelDefinition = serde_json::from_value(value).unwrap();
    ResolvedChannel {
        id: ChannelId::new("cmaeg1.ch.gate-fixture").unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/api/v1/gateway/channel-records/record-1".into(),
        },
        definition,
        agent_key_hash: [0; 32],
        consumer_keys: Vec::new(),
        loaded_at: std::time::Instant::now(),
        version: 1,
    }
}

fn event(channel: &ResolvedChannel, reply: ReplyAddress) -> EventRefPayload {
    EventRefPayload {
        v: 1,
        activity_sha256: None,
        channel_id: channel.id.clone(),
        record_id: channel.pointer.record_id.clone(),
        event_id: "msg-1".parse().unwrap(),
        partition_key: "msg-1".into(),
        conversation_id: "conversation-1".parse().unwrap(),
        expires_at: i64::MAX,
        reply,
        consumer: None,
        lineage: None,
    }
}

fn message() -> OutboundMessage {
    OutboundMessage {
        text: Some("hello".into()),
        blocks: vec![],
        reply_to: None,
    }
}

#[tokio::test]
async fn a_sync_origin_reply_commits_before_its_frame_is_emitted() {
    let channel = channel(|_| {});
    for commits in [false, true] {
        let waiters = Arc::new(SyncRegistry::default());
        let (waiter, frames) = waiters.register().unwrap();
        let delivery = CustomDelivery::new("https://gateway.example".into(), waiters.clone());
        let event = event(
            &channel,
            ReplyAddress::Sync {
                waiter: waiter.clone(),
            },
        );
        let gate = RecordingGate::new(commits);
        let receipt = delivery.reply(&gate, &channel, &event, &message()).await;
        assert_eq!(gate.calls(), ["commit"]);
        // A sentinel marks the end of whatever the reply emitted.
        assert!(waiters.emit(&waiter, "cmaeg.completed", &json!({"sentinel":true}), true));
        let frames: Vec<_> = frames.stream().collect().await;
        if commits {
            assert_eq!(
                receipt.delivery,
                DeliveryOutcome::Confirmed {
                    platform_message_id: None
                }
            );
            assert_eq!(frames.len(), 2, "the frame follows the commit");
        } else {
            assert_eq!(receipt.delivery, expired());
            assert_eq!(frames.len(), 1, "a refused commit emits nothing");
        }
    }
}

/// Accepts every webhook at a public address.
struct Accepting;
impl crate::delivery::webhook_client::WebhookTransport for Accepting {
    fn resolve(
        &self,
        _url: reqwest::Url,
    ) -> BoxFuture<'_, Result<Vec<std::net::SocketAddr>, crate::net::exchange::TransportError>>
    {
        Box::pin(async { Ok(vec!["8.8.8.8:443".parse().unwrap()]) })
    }
    fn post(
        &self,
        _url: reqwest::Url,
        _addresses: Vec<std::net::SocketAddr>,
        _headers: reqwest::header::HeaderMap,
        _body: Arc<Vec<u8>>,
    ) -> BoxFuture<'_, Result<u16, crate::net::exchange::TransportError>> {
        Box::pin(async { Ok(200) })
    }
}

#[tokio::test]
async fn a_webhook_only_reply_commits_inside_its_first_attempt() {
    let channel = channel(|value| {
        value["reply"]["sinks"] = json!([{"type":"result_webhook","url":"https://receiver.example/events","hmac_key_id":"result"}]);
        value["secrets"]["hmac_secrets"] = json!([["result", "result-key"]]);
    });
    let mut delivery = CustomDelivery::new("https://gateway.example".into(), Arc::default());
    delivery.webhooks = WebhookClient::with_transport(Arc::new(Accepting));
    let gate = RecordingGate::new(true);
    let receipt = delivery
        .reply(
            &gate,
            &channel,
            &event(&channel, ReplyAddress::None),
            &message(),
        )
        .await;
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Confirmed {
            platform_message_id: None
        }
    );
    assert_eq!(
        gate.calls(),
        ["enter_route", "attempt", "commit", "progress"]
    );
}
