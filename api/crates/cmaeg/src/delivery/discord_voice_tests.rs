use super::{
    discord_tests::setup,
    gate::recording::RecordingGate,
    intents::IntentReceipt,
    send::{SendContext, Sender},
};
use crate::{
    channels::resolver::ResolvedChannel, discord::rest_fixture::Fixture, media::transform::pcm,
};
use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD, Engine};
use bytes::BytesMut;
use cmaeg_core::{
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::sync::Arc;

fn message() -> OutboundMessage {
    let mut bytes = BytesMut::zeroed(pcm::WAV_HEADER_BYTES);
    pcm::write_wav_header(&mut bytes, 3200);
    bytes.extend_from_slice(&[0; 3200]);
    serde_json::from_value(json!({"native_attachments":[{"kind":"voice","source":{"type":"base64","data":STANDARD.encode(bytes)}}]})).unwrap()
}
pub(super) fn response(channel: &str) -> Value {
    json!({"id":"800","channel_id":channel,"author":{"id":"77","bot":true},"type":19,
        "flags":8192,"content":"","message_reference":{"message_id":"700","channel_id":channel},
        "attachments":[{"id":"801","filename":"voice.wav","content_type":"audio/wav","size":3244,"duration_secs":0.1,"waveform":"AA=="}]})
}
pub(super) async fn send(
    sender: &Sender,
    channel: &Arc<ResolvedChannel>,
    event: &EventRefPayload,
    message: &OutboundMessage,
    allowed: bool,
) -> IntentReceipt {
    let gate = RecordingGate::new(allowed);
    let trace = tracing::Span::none();
    let ctx = SendContext {
        trace: &trace,
        channel,
        first_send_done: false,
        gate: &gate,
    };
    sender.reply(&ctx, event, message).await.0
}

#[tokio::test]
async fn voice_is_independent_default_off_and_exact_native_reply_without_effects_or_controls() {
    let f = Fixture::new().await;
    let (mut sender, channel, mut event) = setup(&f);
    event.activity_sha256 = Some("a".repeat(64));
    sender.relay = sender.relay.with_native_media_presentations(true);
    assert_eq!(
        send(&sender, &channel, &event, &message(), true)
            .await
            .delivery
            .kind(),
        "unsupported"
    );
    assert!(f.memory.calls.lock().unwrap().is_empty());
    sender.relay = sender
        .relay
        .with_native_media_presentations(false)
        .with_native_discord_voice(true)
        .with_native_discord_effects(true)
        .with_native_discord_media_effects(true)
        .with_native_discord_edits(true)
        .with_discord_interactions(true)
        .with_discord_media_controls(true)
        .with_discord_reactions(true);
    for thread in [false, true] {
        let target = if thread { "100" } else { "99" };
        if let ReplyAddress::Discord {
            channel_id,
            thread_id,
            ..
        } = &mut event.reply
        {
            *channel_id = target.into();
            *thread_id = thread.then(|| target.into());
        }
        f.respond(StatusCode::OK, response(target));
        let mut voice = message();
        voice.thread_reply = thread;
        let receipt = send(&sender, &channel, &event, &voice, true).await;
        assert_eq!(receipt.delivery.kind(), "confirmed");
        assert!(receipt.message_ref.is_none() && receipt.message_parts.is_empty());
    }
    let calls = f.memory.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].path, "/api/v10/channels/100/messages");
    let body = String::from_utf8_lossy(&calls[0].body);
    for required in [
        "\"flags\":8192",
        "\"duration_secs\":0.1",
        "\"waveform\":\"AA==\"",
        "\"fail_if_not_exists\":true",
        "Content-Type: audio/wav",
    ] {
        assert!(body.contains(required), "{required}");
    }
    for absent in ["\"content\":", "\"components\":", "\"embeds\":", "files[1]"] {
        assert!(!body.contains(absent));
    }
}

#[tokio::test]
async fn invalid_mix_profile_revocation_and_denied_commit_have_no_send_effect() {
    let f = Fixture::new().await;
    let (mut sender, mut channel, mut event) = setup(&f);
    sender.relay = sender.relay.with_native_discord_voice(true);
    event.activity_sha256 = Some("a".repeat(64));
    for defect in 0..7 {
        let mut raw = serde_json::to_value(message()).unwrap();
        match defect {
            0 => raw["text"] = "text".into(),
            1 => raw["blocks"] = json!([{"type":"text","text":"text"}]),
            2 => raw["native_attachments"][0]["caption"] = "caption".into(),
            3 => {
                let item = raw["native_attachments"][0].clone();
                raw["native_attachments"].as_array_mut().unwrap().push(item);
            }
            4 => raw["native_attachments"][0]["mime_type"] = "audio/ogg".into(),
            5 => raw["native_attachments"][0]["source"]["data"] = "AQIDBA==".into(),
            _ => raw["thread_reply"] = true.into(),
        }
        let voice = serde_json::from_value(raw).unwrap();
        assert_ne!(
            send(&sender, &channel, &event, &voice, true)
                .await
                .delivery
                .kind(),
            "confirmed"
        );
    }
    assert_eq!(
        send(&sender, &channel, &event, &message(), false)
            .await
            .delivery
            .kind(),
        "rejected"
    );
    Arc::make_mut(&mut channel).definition.sources.clear();
    assert_eq!(
        send(&sender, &channel, &event, &message(), true)
            .await
            .delivery
            .kind(),
        "rejected"
    );
    assert!(f.memory.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn wrong_recipient_or_changed_presentation_is_unknown_and_never_automatically_resent() {
    let f = Fixture::new().await;
    let (mut sender, channel, mut event) = setup(&f);
    sender.relay = sender.relay.with_native_discord_voice(true);
    event.activity_sha256 = Some("a".repeat(64));
    for (pointer, value) in [
        ("/channel_id", json!("100")),
        ("/flags", json!(0)),
        ("/attachments/0/waveform", json!("/w==")),
        ("/attachments/0/duration_secs", json!(10)),
        ("/message_reference/message_id", json!("701")),
    ] {
        let mut body = response("99");
        *body.pointer_mut(pointer).unwrap() = value;
        f.respond(StatusCode::OK, body);
        let count = f.memory.calls.lock().unwrap().len();
        let receipt = send(&sender, &channel, &event, &message(), true).await;
        assert_eq!(receipt.delivery.kind(), "unknown");
        assert!(receipt.message_ref.is_none());
        assert_eq!(f.memory.calls.lock().unwrap().len(), count + 1);
    }
    *f.memory.broken.lock().unwrap() = true;
    assert_eq!(
        send(&sender, &channel, &event, &message(), true)
            .await
            .delivery
            .kind(),
        "unknown"
    );
    assert_eq!(f.memory.calls.lock().unwrap().len(), 6);
}

#[tokio::test]
async fn generic_audio_retains_its_old_request_without_voice_fields() {
    let f = Fixture::new().await;
    let (mut sender, channel, event) = setup(&f);
    sender.relay = sender.relay.with_native_discord_voice(true);
    let mut raw = serde_json::to_value(message()).unwrap();
    raw["native_attachments"][0]["kind"] = "audio".into();
    assert_eq!(
        send(
            &sender,
            &channel,
            &event,
            &serde_json::from_value(raw).unwrap(),
            true
        )
        .await
        .delivery
        .kind(),
        "confirmed"
    );
    let calls = f.memory.calls.lock().unwrap();
    let body = String::from_utf8_lossy(&calls[0].body);
    for field in ["\"flags\":", "\"duration_secs\":", "\"waveform\":"] {
        assert!(!body.contains(field));
    }
}
