//! Native media against the real sender with an in-process NyxID transport.
use super::{
    fake_http::{Answer, FakeHttp},
    recording_fixtures::{event, relay_activity, relay_channel, relay_reply, REPLY_TOKEN},
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    delivery::intents::{DeliveryOutcome, ReplySource},
    net::exchange::{HttpReply, TransportError},
    DeliveryConfig,
};
use cmaeg_core::{
    channel::Platform,
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn setup(http: FakeHttp, enabled: bool) -> (Recording, Arc<ResolvedChannel>, EventRefPayload) {
    // Deliberately keep legacy ordinary reply mode; media must still be one request.
    let recording = Recording::start_with_relay(
        http,
        Hooks::answering([]),
        DeliveryConfig::default(),
        None,
        true,
        |relay| relay.with_native_media(enabled),
    );
    let channel = relay_channel(0);
    let event = event(&channel, &relay_activity(), 1, relay_reply());
    (recording, channel, event)
}
fn media() -> Value {
    json!({"native_attachments":[
        {"kind":"image","source":{"type":"base64","data":"iVBORw0KGgo="},"filename":"plot.png","mime_type":"image/png","caption":"Plot"},
        {"kind":"audio","source":{"type":"url","url":"https://media.example/voice.ogg"},"mime_type":"audio/ogg"},
        {"kind":"video","source":{"type":"url","url":"https://media.example/clip.mp4"}},
        {"kind":"file","source":{"type":"base64","data":"aGVsbG8="},"filename":"report.txt","mime_type":"text/plain"}
    ]})
}

#[tokio::test(start_paused = true)]
async fn media_only_transfers_all_kinds_and_sources_in_one_bound_request_on_each_platform() {
    for platform in [
        Platform::Telegram,
        Platform::Lark,
        Platform::Feishu,
        Platform::Discord,
        Platform::Whatsapp,
    ] {
        let (mut recording, channel, mut event) = setup(FakeHttp::relay(), true);
        if let ReplyAddress::NyxidRelay {
            platform: value, ..
        } = &mut event.reply
        {
            *value = platform;
        }
        let message = media();
        serde_json::from_value::<OutboundMessage>(message.clone())
            .unwrap()
            .validate()
            .unwrap();
        let receipt = recording
            .reply(&channel, &event, message.clone(), ReplySource::Tool)
            .await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert!(receipt
            .message_ref
            .unwrap()
            .open(&recording.keys)
            .unwrap()
            .payload
            .relay_message
            .is_none());
        let sent = recording.http.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].json()["message_id"], "inbound-1");
        assert_eq!(sent[0].json()["reply"]["text"], Value::Null);
        assert_eq!(
            sent[0].json()["reply"]["attachments"],
            message["native_attachments"]
        );
        assert_eq!(
            sent[0].header("authorization"),
            Some(format!("Bearer {REPLY_TOKEN}").as_str())
        );
        assert_eq!(sent[0].timeout, Duration::from_secs(60));
        recording.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn explicit_thread_media_keeps_original_target_and_legacy_attachment_blocks_stay_links() {
    let (mut recording, channel, mut event) = setup(FakeHttp::relay(), true);
    if let ReplyAddress::NyxidRelay {
        platform,
        thread_reply,
        ..
    } = &mut event.reply
    {
        *platform = Platform::Lark;
        *thread_reply = true;
    }
    let mut request = media();
    request["thread_reply"] = true.into();
    request["text"] = "hello".into();
    request["blocks"] = json!([{"type":"attachment","reference":"https://media.example/legacy.pdf","kind":"file","caption":"Legacy link"}]);
    let receipt = recording
        .reply(&channel, &event, request, ReplySource::Tool)
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let sent = recording.http.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].url.path(), "/api/v1/channel-relay/thread-reply");
    assert_eq!(sent[0].json()["thread_reply"], true);
    assert_eq!(sent[0].json()["message_id"], "inbound-1");
    assert!(sent[0].json()["reply"]["text"]
        .as_str()
        .unwrap()
        .contains("https://media.example/legacy.pdf"));
    assert_eq!(
        sent[0].json()["reply"]["attachments"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    recording.finish().await;
}

#[tokio::test(start_paused = true)]
async fn unsupported_destinations_disabled_rollout_and_controls_never_send_or_consume_credentials()
{
    for case in ["disabled", "slack", "custom", "controls"] {
        let (mut recording, channel, mut event) = setup(FakeHttp::platforms(), case != "disabled");
        let mut request = media();
        match case {
            "slack" => {
                if let ReplyAddress::NyxidRelay { platform, .. } = &mut event.reply {
                    *platform = Platform::Slack;
                }
            }
            "custom" => event.reply = ReplyAddress::None,
            "controls" => {
                request["blocks"] =
                    json!([{"type":"buttons","id":"choose","options":[{"id":"yes","label":"Yes"}]}])
            }
            _ => {}
        }
        let receipt = recording
            .reply(&channel, &event, request, ReplySource::Tool)
            .await;
        assert_eq!(receipt.delivery, DeliveryOutcome::Unsupported, "{case}");
        assert!(recording.http.sent().is_empty(), "{case}");
        recording.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn unsafe_sources_fail_before_dispatch_and_leave_the_single_use_token_available() {
    let (mut recording, channel, event) = setup(FakeHttp::relay(), true);
    for url in [
        "https://127.0.0.1/a",
        "https://[::1]/a",
        "https://169.254.169.254/a",
        "https://10.0.0.1/a",
        "https://localhost/a",
        "https://metadata.google.internal/a",
        "https://service.namespace.svc/a",
        "https://[::ffff:127.0.0.1]/a",
    ] {
        let request =
            json!({"native_attachments":[{"kind":"file","source":{"type":"url","url":url}}]});
        let receipt = recording
            .reply(&channel, &event, request, ReplySource::Tool)
            .await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Rejected { .. }),
            "{url}"
        );
        assert!(recording.http.sent().is_empty());
    }
    let receipt = recording
        .reply(&channel, &event, media(), ReplySource::Tool)
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(
        recording.http.sent()[0].header("authorization"),
        Some(format!("Bearer {REPLY_TOKEN}").as_str())
    );
    recording.finish().await;
}

#[tokio::test(start_paused = true)]
async fn uncertain_or_partial_media_send_is_never_retried_or_confirmed() {
    let lost = Answer {
        reply: Err(TransportError::Unavailable),
        delay: Duration::ZERO,
    };
    let truncated = Answer {
        reply: Ok(HttpReply::new(
            reqwest::StatusCode::OK,
            br#"{"message_id":"unfinished""#.to_vec(),
        )),
        delay: Duration::ZERO,
    };
    for answer in [
        Answer::status(429).retry_after("1"),
        Answer::status(502),
        lost,
        truncated,
        Answer::json(200, json!({"message_id":"sent-no-platform-receipt"})),
    ] {
        let (mut recording, channel, event) = setup(FakeHttp::relay().script([answer]), true);
        let receipt = recording
            .reply(&channel, &event, media(), ReplySource::Tool)
            .await;
        assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        assert!(receipt.message_ref.is_none());
        assert_eq!(recording.http.sent().len(), 1);
        recording.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn media_uploads_have_a_bounded_longer_timeout_without_changing_text_delivery() {
    let accepted = Answer::json(
        200,
        json!({"message_id":"complete","platform_message_id":"media-1"}),
    );
    let (mut recording, channel, event) = setup(
        FakeHttp::relay().script([accepted.after(Duration::from_secs(12))]),
        true,
    );
    let receipt = recording
        .reply(&channel, &event, media(), ReplySource::Tool)
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    let text = recording
        .reply(
            &channel,
            &event,
            json!({"text":"ordinary"}),
            ReplySource::Tool,
        )
        .await;
    assert!(matches!(text.delivery, DeliveryOutcome::Confirmed { .. }));
    assert_eq!(recording.http.sent()[0].timeout, Duration::from_secs(60));
    assert_eq!(recording.http.sent()[1].timeout, Duration::from_secs(5));
    assert!(recording.http.sent()[1].json()["reply"]
        .get("attachments")
        .is_none());
    recording.finish().await;
    let delayed = Answer::json(
        200,
        json!({"message_id":"late","platform_message_id":"media-2"}),
    )
    .after(Duration::from_secs(61));
    let (mut recording, channel, event) = setup(FakeHttp::relay().script([delayed]), true);
    let receipt = recording
        .reply(&channel, &event, media(), ReplySource::Tool)
        .await;
    assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
    assert_eq!(recording.http.sent().len(), 1);
    recording.finish().await;
}
