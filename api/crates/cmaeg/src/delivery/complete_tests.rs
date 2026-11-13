use super::{
    gate::recording::RecordingGate,
    intents::DeliveryOutcome,
    pacing::Pacing,
    relay_client::{RelayClient, RelayTransport, REQUEST_MAX_BYTES},
    send::{SendContext, Sender},
};
mod commonmark;
mod destinations;
mod durable;
mod native_lark_control_tests;
mod native_lark_document_tests;
mod native_lark_image_replacement_tests;
mod native_lark_media_fixture;
mod native_lark_media_tests;
mod native_lark_multi_image_tests;
mod native_lark_sticker_tests;
mod native_lark_tests;
pub(crate) mod native_lark_thread_start_fixture;
mod native_lark_thread_start_tests;
mod native_whatsapp_control_tests;
mod native_whatsapp_document_tests;
mod native_whatsapp_group_tests;
mod native_whatsapp_media_control_tests;
mod native_whatsapp_sticker_tests;
mod native_whatsapp_template_tests;
mod native_whatsapp_tests;
mod native_whatsapp_video_tests;
mod whatsapp;
use crate::{
    channels::resolver::ResolvedChannel,
    net::exchange::{HttpReply, TransportError},
    turns::observer::NoopObserver,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    channel::{ChannelDefinition, Platform, Secret},
    message::OutboundMessage,
    seal::{ChannelPointer, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, StatusCode, Url};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct Transport(Mutex<Vec<usize>>, Mutex<Vec<(Url, serde_json::Value)>>);
impl RelayTransport for Transport {
    fn post(
        &self,
        url: Url,
        _: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.0.lock().unwrap().push(body.len());
        self.1
            .lock()
            .unwrap()
            .push((url, serde_json::from_slice(&body).unwrap()));
        Box::pin(async {
            Ok(HttpReply::new(
                StatusCode::OK,
                br#"{"message_id":"out-1","platform_message_id":"p-1"}"#.to_vec(),
            ))
        })
    }
}
fn setup() -> (
    Sender,
    Arc<Transport>,
    Arc<ResolvedChannel>,
    EventRefPayload,
) {
    let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    definition.secrets.channel_key = None;
    let channel = Arc::new(ResolvedChannel {
        id: "cmaeg1.ch.complete-test".parse().unwrap(),
        pointer: ChannelPointer {
            v: 1,
            record_id: definition.record_id.clone(),
            record_url: "https://cma.example/record".into(),
        },
        definition,
        agent_key_hash: [0; 32],
        consumer_keys: vec![],
        loaded_at: std::time::Instant::now(),
        version: 1,
    });
    let event = EventRefPayload {
        telegram_admission: None,
        v: 1,
        activity_sha256: None,
        channel_id: channel.id.clone(),
        record_id: channel.pointer.record_id.clone(),
        event_id: "event-1".parse().unwrap(),
        partition_key: "partition-1".into(),
        conversation_id: "conversation-1".parse().unwrap(),
        expires_at: i64::MAX,
        reply: ReplyAddress::NyxidRelay {
            thread_reply: false,
            message_id: "inbound\\\"中文".into(),
            route_id: "route-1".into(),
            platform: Platform::Telegram,
            reply_token: Secret::new("single-use-test-fixture"),
            reply_token_expires_at: i64::MAX,
        },
        consumer: None,
        lineage: None,
    };
    let transport = Arc::new(Transport::default());
    let sender = Sender {
        custom: super::custom::CustomDelivery::new(
            "https://gateway.example".into(),
            Arc::default(),
        ),
        keys: Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([7; 32]))).unwrap()),
        relay: RelayClient::new("https://nyx.example", Duration::from_secs(5))
            .unwrap()
            .with_complete_replies(true)
            .with_transport(transport.clone()),
        pacing: Pacing::default(),
        consumed: Mutex::default(),
        observer: Arc::new(NoopObserver),
    };
    (sender, transport, channel, event)
}
fn message(text: String) -> OutboundMessage {
    OutboundMessage {
        whatsapp_template: None,
        structured_content: None,
        reply_with_received_sticker: false,
        clear_native_attachments: false,
        native_media_update: None,
        native_attachments: Vec::new(),
        thread_reply: false,
        text: Some(text),
        blocks: vec![],
        reply_to: None,
    }
}

#[tokio::test(start_paused = true)]
async fn exact_serialized_boundary_is_accepted_and_one_more_byte_is_refused_before_gate_or_claim() {
    let (sender, transport, channel, event) = setup();
    let ReplyAddress::NyxidRelay { message_id, .. } = &event.reply else {
        unreachable!()
    };
    let envelope =
        serde_json::to_vec(&json!({"message_id":message_id,"reply":{"text":"","metadata":null}}))
            .unwrap()
            .len();
    // Quotes expand in JSON; the identifier's own escapes also count.
    let remaining = REQUEST_MAX_BYTES - envelope;
    let text = format!(
        "{}{}",
        "\"".repeat(remaining / 2),
        "x".repeat(remaining % 2)
    );
    let trace = tracing::Span::none();
    let gate = RecordingGate::new(true);
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, effects) = sender
        .reply(&ctx, &event, &message(format!("{text}x")))
        .await;
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Rejected {
            reason: "relay_request_too_large".into()
        }
    );
    assert!(!effects.first_send_done);
    assert!(gate.calls().is_empty());
    assert!(sender.consumed.lock().unwrap().is_empty());
    assert!(transport.0.lock().unwrap().is_empty());
    let (receipt, effects) = sender.reply(&ctx, &event, &message(text)).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert!(effects.first_send_done);
    assert_eq!(
        gate.calls(),
        ["enter_route", "attempt", "commit", "progress"]
    );
    assert_eq!(*transport.0.lock().unwrap(), [REQUEST_MAX_BYTES]);
    assert_eq!(sender.consumed.lock().unwrap().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_commit_refuses_before_token_claim_and_http() {
    let (sender, transport, channel, event) = setup();
    let trace = tracing::Span::none();
    let gate = RecordingGate::new(false);
    let ctx = SendContext {
        trace: &trace,
        channel: &channel,
        first_send_done: false,
        gate: &gate,
    };
    let (receipt, effects) = sender.reply(&ctx, &event, &message("x".repeat(9000))).await;
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Rejected {
            reason: "delivery_expired".into()
        }
    );
    assert!(!effects.first_send_done);
    assert_eq!(gate.calls(), ["enter_route", "attempt", "commit"]);
    assert!(sender.consumed.lock().unwrap().is_empty());
    assert!(transport.0.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn complete_replies_keep_thread_choice_and_sealed_target_independent() {
    for (available, requested) in [(true, true), (true, false), (false, true)] {
        let (sender, transport, channel, mut event) = setup();
        if let ReplyAddress::NyxidRelay {
            thread_reply,
            platform,
            ..
        } = &mut event.reply
        {
            *thread_reply = available;
            *platform = Platform::Lark;
        }
        let trace = tracing::Span::none();
        let gate = RecordingGate::new(true);
        let ctx = SendContext {
            trace: &trace,
            channel: &channel,
            first_send_done: false,
            gate: &gate,
        };
        let mut message = message("long complete response ".repeat(1000));
        message.thread_reply = requested;
        let (receipt, effects) = sender.reply(&ctx, &event, &message).await;
        if !available {
            assert_eq!(
                receipt.delivery,
                DeliveryOutcome::Rejected {
                    reason: "thread_target_unavailable".into()
                }
            );
            assert!(!effects.first_send_done);
            assert!(gate.calls().is_empty());
            assert!(sender.consumed.lock().unwrap().is_empty());
            assert!(transport.1.lock().unwrap().is_empty());
            continue;
        }
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        let requests = transport.1.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let (url, body) = &requests[0];
        assert_eq!(
            url.path(),
            if requested {
                "/api/v1/channel-relay/thread-reply"
            } else {
                "/api/v1/channel-relay/reply"
            }
        );
        assert_eq!(
            body.get("thread_reply"),
            requested.then_some(&serde_json::Value::Bool(true))
        );
        assert_eq!(body["reply"]["text"], message.text.unwrap());
    }
}
