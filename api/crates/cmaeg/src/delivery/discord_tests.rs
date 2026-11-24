use super::{
    gate::recording::RecordingGate,
    intents::DeliveryOutcome,
    pacing::Pacing,
    relay_client::RelayClient,
    send::{SendContext, Sender},
};
use crate::{
    channels::resolver::ResolvedChannel, discord::rest_fixture::Fixture,
    turns::observer::NoopObserver,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::{
    channel::{AdmissionPolicy, ChannelDefinition, Secret, Source},
    message::OutboundMessage,
    seal::{ChannelPointer, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
pub(super) fn setup(f: &Fixture) -> (Sender, Arc<ResolvedChannel>, EventRefPayload) {
    let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    definition.sources = vec![Source::Discord {
        thread_history: false,
        thread_lifecycle: false,
        conversation_history: false,
        bot_id: "77".into(),
        intents: 37377,
        admission: AdmissionPolicy::All {},
    }];
    definition.secrets.discord_bot_token = Some(Secret::new("native-local-fixture"));
    let channel = Arc::new(ResolvedChannel {
        id: "cmaeg1.ch.discord-fixture".parse().unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://records.example/channel".into(),
        },
        definition,
        agent_key_hash: [7; 32],
        consumer_keys: vec![],
        loaded_at: Instant::now(),
        version: 1,
    });
    let event = EventRefPayload {
        telegram_admission: None,
        v: 1,
        activity_sha256: None,
        channel_id: channel.id.clone(),
        record_id: channel.pointer.record_id.clone(),
        event_id: "discord:77:700".parse().unwrap(),
        partition_key: "discord:99".into(),
        conversation_id: "conversation-1".parse().unwrap(),
        expires_at: i64::MAX,
        reply: ReplyAddress::Discord {
            bot_id: "77".into(),
            channel_id: "99".into(),
            message_id: "700".into(),
            thread_id: None,
        },
        consumer: None,
        lineage: None,
    };
    let mut relay = RelayClient::new("https://nyx.example", Duration::from_secs(5)).unwrap();
    relay.discord = f.api.clone();
    let sender = Sender {
        custom: super::custom::CustomDelivery::new(
            "https://gateway.example".into(),
            Arc::default(),
        ),
        keys: Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([7; 32]))).unwrap()),
        relay,
        pacing: Pacing::default(),
        consumed: Mutex::default(),
        observer: Arc::new(NoopObserver),
    };
    (sender, channel, event)
}
#[tokio::test]
async fn native_discord_delivery_uses_gate_and_sends_files_without_relay_media_flag() {
    let f = Fixture::new().await;
    let (sender, channel, event) = setup(&f);
    let message:OutboundMessage=serde_json::from_value(json!({"text":"hello","native_attachments":[{"kind":"file","source":{"type":"base64","data":"AQIDBA=="},"filename":"file.bin"}]})).unwrap();
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, effects) = sender.reply(&ctx, &event, &message).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert!(!effects.first_send_done);
    assert_eq!(
        gate.calls(),
        ["enter_route", "attempt", "commit", "progress"]
    );
    assert_eq!(f.memory.calls.lock().unwrap().len(), 1);
    assert!(sender.consumed.lock().unwrap().is_empty());
}
#[tokio::test]
async fn native_discord_denied_commit_and_revoked_source_never_send() {
    let f = Fixture::new().await;
    let (sender, mut channel, event) = setup(&f);
    let message: OutboundMessage = serde_json::from_value(json!({"text":"hello"})).unwrap();
    let gate = RecordingGate::new(false);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, _) = sender.reply(&ctx, &event, &message).await;
    assert!(
        matches!(receipt.delivery,DeliveryOutcome::Rejected{reason} if reason=="delivery_expired")
    );
    Arc::get_mut(&mut channel)
        .unwrap()
        .definition
        .sources
        .clear();
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, _) = sender.reply(&ctx, &event, &message).await;
    assert!(
        matches!(receipt.delivery,DeliveryOutcome::Rejected{reason} if reason=="native_destination_revoked")
    );
    assert!(f.memory.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn native_discord_long_unicode_response_is_split_without_losing_content() {
    let f = Fixture::new().await;
    let (sender, channel, event) = setup(&f);
    let text = "😀".repeat(1001);
    let message: OutboundMessage = serde_json::from_value(json!({"text":text})).unwrap();
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    assert!(matches!(
        sender.reply(&ctx, &event, &message).await.0.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let calls = f.memory.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    let parts: Vec<String> = calls
        .iter()
        .map(|call| {
            serde_json::from_slice::<serde_json::Value>(&call.body).unwrap()["content"]
                .as_str()
                .unwrap()
                .into()
        })
        .collect();
    assert_eq!(parts.concat(), text);
    assert!(parts.iter().all(|p| p.encode_utf16().count() <= 2000));
}
#[tokio::test]
async fn native_discord_keeps_markdown_links_and_neutralizes_mentions() {
    let f = Fixture::new().await;
    let (sender, channel, event) = setup(&f);
    let text = "([nict.go.jp](https://www.nict.go.jp/en/sts/jst.html?utm_source=openai)) \
                C:\\qa\\menu_01.txt @everyone <@1>";
    let message: OutboundMessage = serde_json::from_value(json!({"text":text})).unwrap();
    let gate = RecordingGate::new(true);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    assert!(matches!(
        sender.reply(&ctx, &event, &message).await.0.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let calls = f.memory.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let body = serde_json::from_slice::<serde_json::Value>(&calls[0].body).unwrap();
    assert_eq!(
        body["content"],
        "([nict.go.jp](https://www.nict.go.jp/en/sts/jst.html?utm_source=openai)) \
         C:\\qa\\menu_01.txt @\u{200b}everyone ‹@\u{200b}1›"
    );
}
