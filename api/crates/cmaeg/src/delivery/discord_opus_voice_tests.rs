//! The encoded upload is unchanged; preparation never grants a different destination.
use super::{
    discord_tests::setup,
    discord_voice_tests::{response, send},
};
use crate::discord::{
    rest_fixture::Fixture,
    voice::test_support::{decoder, Output, OGG},
};
use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg_core::message::OutboundMessage;
use serde_json::{json, Value};

fn message() -> OutboundMessage {
    serde_json::from_value(json!({"native_attachments":[{"kind":"voice","source":{"type":"base64","data":STANDARD.encode(OGG)}}]})).unwrap()
}
fn confirmed() -> Value {
    let mut body = response("99");
    body["attachments"][0] = json!({"id":"801","filename":"voice.ogg","content_type":"audio/ogg",
        "size":OGG.len(),"duration_secs":0.3,"waveform":"AP+A"});
    body
}

#[tokio::test]
async fn opus_uses_decoder_and_sends_original_encoded_bytes_once_to_exact_target() {
    let f = Fixture::new().await;
    let (mut sender, channel, mut event) = setup(&f);
    event.activity_sha256 = Some("a".repeat(64));
    let (decoder, stub, _) = decoder(Output::Good);
    sender.relay = sender
        .relay
        .with_native_discord_voice(true)
        .with_discord_voice_decoder(Some(decoder));
    f.respond(StatusCode::OK, confirmed());
    let receipt = send(&sender, &channel, &event, &message(), true).await;
    assert_eq!(receipt.delivery.kind(), "confirmed");
    assert!(receipt.message_ref.is_none());
    assert_eq!(stub.calls.lock().unwrap().len(), 1);
    let calls = f.memory.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].path, "/api/v10/channels/99/messages");
    assert!(calls[0].body.windows(OGG.len()).any(|bytes| bytes == OGG));
    let body = String::from_utf8_lossy(&calls[0].body);
    assert!(body.contains("Content-Type: audio/ogg"));
    assert!(body.contains("\"duration_secs\":0.3"));
    assert!(body.contains("\"waveform\":\"AP+A\""));
    assert!(!body.contains("Content-Type: audio/wav"));
}

#[tokio::test]
async fn opus_missing_or_failed_decoder_and_denied_authority_never_post() {
    let f = Fixture::new().await;
    let (mut sender, channel, mut event) = setup(&f);
    event.activity_sha256 = Some("a".repeat(64));
    sender.relay = sender.relay.with_native_discord_voice(true);
    assert_eq!(
        send(&sender, &channel, &event, &message(), true)
            .await
            .delivery
            .kind(),
        "rejected"
    );
    for outcome in [Output::Failed, Output::Truncated, Output::AtLimit] {
        let (decoder, _, _) = decoder(outcome);
        sender.relay = sender.relay.with_discord_voice_decoder(Some(decoder));
        assert_eq!(
            send(&sender, &channel, &event, &message(), true)
                .await
                .delivery
                .kind(),
            "rejected"
        );
    }
    let (decoder, _, _) = decoder(Output::Good);
    sender.relay = sender.relay.with_discord_voice_decoder(Some(decoder));
    assert_eq!(
        send(&sender, &channel, &event, &message(), false)
            .await
            .delivery
            .kind(),
        "rejected"
    );
    assert!(f.memory.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn opus_inconsistent_platform_acceptance_stays_unknown_without_a_second_send() {
    for defect in ["destination", "mime", "duration", "waveform", "size"] {
        let f = Fixture::new().await;
        let (mut sender, channel, mut event) = setup(&f);
        event.activity_sha256 = Some("a".repeat(64));
        let (decoder, _, _) = decoder(Output::Good);
        sender.relay = sender
            .relay
            .with_native_discord_voice(true)
            .with_discord_voice_decoder(Some(decoder));
        let mut body = confirmed();
        match defect {
            "destination" => body["channel_id"] = "100".into(),
            "mime" => body["attachments"][0]["content_type"] = "audio/wav".into(),
            "duration" => body["attachments"][0]["duration_secs"] = 4.into(),
            "waveform" => body["attachments"][0]["waveform"] = "AA==".into(),
            "size" => body["attachments"][0]["size"] = 1.into(),
            _ => unreachable!(),
        }
        f.respond(StatusCode::OK, body);
        let receipt = send(&sender, &channel, &event, &message(), true).await;
        assert_eq!(receipt.delivery.kind(), "unknown", "{defect}");
        assert!(receipt.message_ref.is_none());
        assert_eq!(f.memory.calls.lock().unwrap().len(), 1);
    }
}
