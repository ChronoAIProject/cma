use super::{
    gate::recording::RecordingGate,
    intents::DeliveryOutcome,
    pacing::Pacing,
    relay_client::RelayClient,
    send::{SendContext, Sender},
};
use crate::{
    channels::resolver::ResolvedChannel,
    telegram::media_fixture::{Answer, Fixture},
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
#[path = "telegram_album_tests.rs"]
mod albums;
#[path = "telegram_media_effect_tests.rs"]
mod effects;
pub(super) fn setup(f: &Fixture) -> (Sender, Arc<ResolvedChannel>, EventRefPayload) {
    let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    definition.sources = vec![Source::Telegram {
        topic_lifecycle: false,
        topic_creation: false,
        message_events: false,
        channel_posts: false,
        member_join_chat_ids: Vec::new(),
        bot_id: "77".into(),
        bot_username: None,
        admission: AdmissionPolicy::All {},
    }];
    definition.secrets.telegram_bot_token = Some(Secret::new("77:fixture"));
    let channel = Arc::new(ResolvedChannel {
        id: "cmaeg1.ch.telegram-media-fixture".parse().unwrap(),
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
        event_id: "telegram:77:700".parse().unwrap(),
        partition_key: "telegram:-42".into(),
        conversation_id: "conversation-1".parse().unwrap(),
        expires_at: i64::MAX,
        reply: ReplyAddress::Telegram {
            bot_id: "77".into(),
            chat_id: "-42".into(),
            thread_id: Some("7".into()),
            message_id: "700".into(),
        },
        consumer: None,
        lineage: None,
    };
    let mut relay = RelayClient::new("https://nyx.example", Duration::from_secs(5)).unwrap();
    relay.telegram = f.api.clone();
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
fn message() -> OutboundMessage {
    serde_json::from_value(json!({"native_attachments":[{"kind":"file","source":{"type":"base64","data":"AQIDBA=="},"filename":"report.bin"}]})).unwrap()
}
async fn send(
    sender: &Sender,
    channel: &Arc<ResolvedChannel>,
    event: &EventRefPayload,
    message: &OutboundMessage,
    gate: &dyn super::gate::SendGate,
) -> super::intents::IntentReceipt {
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel,
        first_send_done: false,
        gate,
    };
    let (receipt, effects) = sender.reply(&ctx, event, message).await;
    assert!(!effects.first_send_done);
    assert!(sender.consumed.lock().unwrap().is_empty());
    receipt
}

#[tokio::test]
async fn native_telegram_media_batch_is_gated_per_effect_without_relay_flag_or_edit_authority() {
    let f = Fixture::new().await;
    let (sender, channel, event) = setup(&f);
    let mut message = message();
    message.text = Some("Text stays separate from captions".into());
    message.native_attachments[0].caption = Some("File caption".into());
    let gate = RecordingGate::new(true);
    let receipt = send(&sender, &channel, &event, &message, &gate).await;
    assert!(
        matches!(receipt.delivery, DeliveryOutcome::Confirmed { platform_message_id: Some(ref id) } if id == "800")
    );
    assert!(receipt.message_ref.is_none());
    assert_eq!(
        gate.calls(),
        [
            "enter_route",
            "attempt",
            "commit",
            "progress",
            "attempt",
            "commit",
            "progress"
        ]
    );
    let calls = f.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].path, "/bot77:fixture/sendMessage");
    assert_eq!(calls[1].path, "/bot77:fixture/sendDocument");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&calls[0].body).unwrap()["text"],
        "Text stays separate from captions"
    );
    assert!(String::from_utf8_lossy(&calls[1].body).contains("File caption"));
}

#[tokio::test]
async fn native_telegram_media_only_has_no_placeholder_text_or_message_reference() {
    let f = Fixture::new().await;
    let (sender, channel, event) = setup(&f);
    let gate = RecordingGate::new(true);
    let receipt = send(&sender, &channel, &event, &message(), &gate).await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert!(receipt.message_ref.is_none());
    assert_eq!(f.calls().len(), 1);
    assert_eq!(f.calls()[0].path, "/bot77:fixture/sendDocument");
}

#[tokio::test]
async fn native_telegram_media_all_attachments_are_prepared_before_even_text_is_sent() {
    let f = Fixture::new().await;
    let (sender, channel, event) = setup(&f);
    let mut message = message();
    message.text = Some("Must not be sent".into());
    let mut unsafe_item = message.native_attachments[0].clone();
    unsafe_item.source = cmaeg_core::message::NativeMediaSource::Url {
        url: "https://127.0.0.1/private".into(),
    };
    message.native_attachments.push(unsafe_item);
    let gate = RecordingGate::new(true);
    assert!(
        matches!(send(&sender, &channel, &event, &message, &gate).await.delivery,
        DeliveryOutcome::Rejected { ref reason } if reason == "attachment_preparation_failed")
    );
    assert!(f.calls().is_empty());
    assert!(gate.calls().is_empty());
}

#[tokio::test]
async fn native_telegram_media_denied_commit_revoked_source_missing_key_and_mismatched_bot_never_send(
) {
    let f = Fixture::new().await;
    for case in ["commit", "source", "credential", "bot"] {
        let (sender, mut channel, event) = setup(&f);
        match case {
            "source" => Arc::get_mut(&mut channel)
                .unwrap()
                .definition
                .sources
                .clear(),
            "credential" => {
                Arc::get_mut(&mut channel)
                    .unwrap()
                    .definition
                    .secrets
                    .telegram_bot_token = None
            }
            "bot" => {
                Arc::get_mut(&mut channel)
                    .unwrap()
                    .definition
                    .secrets
                    .telegram_bot_token = Some(Secret::new("88:fixture"))
            }
            _ => {}
        }
        let gate = RecordingGate::new(case != "commit");
        assert!(matches!(
            send(&sender, &channel, &event, &message(), &gate)
                .await
                .delivery,
            DeliveryOutcome::Rejected { .. }
        ));
        assert!(f.calls().is_empty(), "{case}");
    }
}

#[tokio::test]
async fn native_telegram_media_partial_delivery_is_unknown_and_does_not_retry_or_grant_effects() {
    let f = Fixture::new().await;
    let (sender, channel, event) = setup(&f);
    let mut message = message();
    message.text = Some("first effect".into());
    f.answer(Answer::ok());
    f.answer(Answer::json(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        json!({"ok":false,"error_code":429,"parameters":{"retry_after":1}}),
    ));
    let receipt = send(
        &sender,
        &channel,
        &event,
        &message,
        &RecordingGate::new(true),
    )
    .await;
    assert!(
        matches!(receipt.delivery, DeliveryOutcome::Unknown { ref reason } if reason == "partial_delivery")
    );
    assert!(receipt.message_ref.is_none());
    assert_eq!(f.calls().len(), 2);
}

#[tokio::test]
async fn native_telegram_media_first_write_throttle_and_failure_are_not_retried() {
    for (status, expected) in [
        (reqwest::StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
        (reqwest::StatusCode::BAD_GATEWAY, "unknown"),
    ] {
        let f = Fixture::new().await;
        let (sender, channel, event) = setup(&f);
        f.answer(Answer::json(
            status,
            json!({"ok":false,"parameters":{"retry_after":1}}),
        ));
        let receipt = send(
            &sender,
            &channel,
            &event,
            &message(),
            &RecordingGate::new(true),
        )
        .await;
        assert_eq!(receipt.delivery.kind(), expected);
        assert_eq!(f.calls().len(), 1);
    }
}

#[tokio::test]
async fn native_telegram_media_refuses_controls_or_unproved_explicit_thread_override() {
    let f = Fixture::new().await;
    let (sender, channel, event) = setup(&f);
    for extra in [
        json!({"thread_reply":true}),
        json!({"blocks":[{"type":"buttons","id":"choice","options":[{"id":"yes","label":"Yes"}]}]}),
    ] {
        let mut value = serde_json::to_value(message()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let message = serde_json::from_value(value).unwrap();
        assert_eq!(
            send(
                &sender,
                &channel,
                &event,
                &message,
                &RecordingGate::new(true)
            )
            .await
            .delivery,
            DeliveryOutcome::Unsupported
        );
        assert!(f.calls().is_empty());
    }
}

struct RevokeAfterFirst(std::sync::atomic::AtomicUsize);
impl super::gate::SendGate for RevokeAfterFirst {
    fn enter_route<'a>(
        &'a self,
        _: &'a str,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<super::gate::RouteTicket, super::gate::GateRefusal>,
    > {
        Box::pin(async { Ok(super::gate::RouteTicket::empty()) })
    }
    fn commit(&self) -> bool {
        true
    }
    fn attempt<'a>(
        &'a self,
    ) -> futures_util::future::BoxFuture<
        'a,
        Result<super::gate::AttemptPermit, super::gate::GateRefusal>,
    > {
        Box::pin(async move {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok(super::gate::AttemptPermit::empty())
            } else {
                Err(super::gate::GateRefusal {
                    reason: "source_revoked",
                    retry_after: None,
                })
            }
        })
    }
    fn progress(&self) {}
}
#[tokio::test]
async fn native_telegram_media_rechecks_authority_before_every_effect_and_stops_on_revocation() {
    let f = Fixture::new().await;
    let (sender, channel, event) = setup(&f);
    let mut message = message();
    message
        .native_attachments
        .push(message.native_attachments[0].clone());
    let gate = RevokeAfterFirst(std::sync::atomic::AtomicUsize::new(0));
    let receipt = send(&sender, &channel, &event, &message, &gate).await;
    assert!(
        matches!(receipt.delivery, DeliveryOutcome::Unknown { ref reason } if reason == "partial_delivery")
    );
    assert_eq!(f.calls().len(), 1);
}

#[path = "telegram_media_presentation_tests.rs"]
mod presentations;
