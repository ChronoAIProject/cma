//! Complete mode against the shared fake transport and paused-clock engine.
//! These tests do not prove that a real NyxID or platform has been upgraded.
use super::{
    fake_http::{Answer, FakeHttp},
    recording_fixtures::{
        event, native_channel, relay_activity, relay_channel, relay_reply, turn, CHANNEL_KEY,
        REPLY_TOKEN,
    },
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    channels::resolver::ResolvedChannel,
    delivery::{
        intents::{DeliveryOutcome, ReplySource},
        relay_client::REQUEST_MAX_BYTES,
        render::{RelayText, Renderer},
    },
    net::exchange::{BodyError, HttpReply, TransportError},
    tools::Capabilities,
    turns::outcomes::TurnOutcome,
    DeliveryConfig,
};
use cmaeg_core::{
    channel::Platform,
    message::{OutboundMessage, RenderingOutcome},
    seal::{EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

fn start(http: FakeHttp, config: DeliveryConfig) -> Recording {
    Recording::start_with_relay(http, Hooks::answering([]), config, None, true, |relay| {
        relay.with_complete_replies(true)
    })
}
fn setup(http: FakeHttp) -> (Recording, Arc<ResolvedChannel>, EventRefPayload) {
    let recording = start(http, DeliveryConfig::default());
    let channel = relay_channel(0);
    let event = event(&channel, &relay_activity(), 1, relay_reply());
    (recording, channel, event)
}

#[tokio::test(start_paused = true)]
async fn long_unicode_url_code_and_every_block_remain_in_one_logical_request() {
    let (mut recording, channel, event) = setup(FakeHttp::relay());
    let url = format!(
        "https://example.com/{}?query=value#fragment",
        "segment".repeat(900)
    );
    let message = json!({"text":format!("{}\n{}\n```rust\nlet x = [1, 2];\n```\n@all <tag>\u{202e}\u{0000}", "中文🦀".repeat(1200), url), "blocks":[
        {"type":"text","text":"More _text_"},
        {"type":"card","title":"Title","body":"Body","fields":[{"label":"Name","value":"Value"}],"image":"https://example.com/image.png"},
        {"type":"buttons","id":"buttons","options":[{"id":"yes","label":"Yes"}]},
        {"type":"choice","id":"choice","prompt":"Choose","multi":true,"options":[{"id":"one","label":"One"}]},
        {"type":"form","id":"form","fields":[{"id":"name","label":"Name","kind":{"type":"text"},"required":true}]},
        {"type":"attachment","kind":"file","reference":"https://example.com/report.pdf","caption":"Report"},
        {"type":"link","url":"https://example.com/","label":"More"}
    ]});
    let outbound: OutboundMessage = serde_json::from_value(message.clone()).unwrap();
    outbound.validate().unwrap();
    let mut capabilities = Capabilities::relay(Platform::Telegram);
    capabilities.buttons = "numbered_text".into();
    let (legacy, _) = RelayText.render(&outbound, &capabilities);
    let expected = legacy.into_iter().map(|part| part.text).collect::<String>();
    let receipt = recording
        .reply(&channel, &event, message, ReplySource::Tool)
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(receipt.rendering.outcome, RenderingOutcome::Degraded);
    assert_eq!(receipt.rendering.blocks.len(), 7);
    let sent = recording.http.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].json()["reply"]["text"], expected);
    assert!(expected.contains(&url));
    assert!(expected.contains("\\`\\`\\`rust\nlet x = \\[1, 2\\];"));
    assert!(expected.contains("@\u{200b}all ‹tag›"));
    assert!(!expected.contains(['\u{202e}', '\u{0000}']));
    assert_eq!(
        sent[0].header("authorization"),
        Some(format!("Bearer {REPLY_TOKEN}").as_str())
    );
    assert_eq!(sent[0].json()["reply"]["metadata"], Value::Null);
    assert_eq!(sent[0].timeout, Duration::from_secs(5));
    assert_eq!(
        receipt
            .message_ref
            .unwrap()
            .open(&recording.keys)
            .unwrap()
            .payload
            .platform_message_id,
        "platform-1"
    );
    recording.finish().await;
}

#[tokio::test(start_paused = true)]
async fn maximum_tool_ingress_and_automatic_text_fit_after_escaping_expansion() {
    let (mut recording, channel, event) = setup(FakeHttp::relay());
    let message = json!({"text":"@".repeat(32768),"blocks":(0..16).map(|_| json!({"type":"text","text":"@".repeat(5900)})).collect::<Vec<_>>()});
    let outbound: OutboundMessage = serde_json::from_value(message.clone()).unwrap();
    outbound.validate().unwrap();
    assert!(serde_json::to_vec(&message).unwrap().len() <= 128 * 1024);
    let receipt = recording
        .reply(&channel, &event, message, ReplySource::Tool)
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(receipt.rendering.outcome, RenderingOutcome::Rendered);
    assert_eq!(receipt.rendering.blocks.len(), 16);
    let mut automatic_activity = relay_activity();
    automatic_activity.event_id = "auto-event".parse().unwrap();
    let turn = turn(
        &recording.keys,
        &channel,
        automatic_activity,
        &EventRefPayload {
            event_id: "auto-event".parse().unwrap(),
            ..event
        },
    );
    recording
        .outcome(
            &turn,
            TurnOutcome::Completed {
                final_text: Some("@".repeat(256 * 1024)),
            },
        )
        .await;
    let result = recording.finish().await;
    let calls = result["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[1]["body"]["reply"]["text"].as_str().unwrap(),
        "@\u{200b}".repeat(256 * 1024)
    );
    assert!(serde_json::to_vec(&calls[1]["body"]).unwrap().len() < REQUEST_MAX_BYTES);
}

#[tokio::test(start_paused = true)]
async fn oversize_aggregate_and_sanitized_empty_messages_preserve_the_reply_token() {
    let (mut recording, mut channel, event) = setup(FakeHttp::relay());
    Arc::get_mut(&mut channel)
        .unwrap()
        .definition
        .secrets
        .channel_key = None;
    let oversized = json!({"text":"@".repeat(32768),"blocks":(0..16).map(|_| json!({"type":"text","text":"@".repeat(32768)})).collect::<Vec<_>>()});
    serde_json::from_value::<OutboundMessage>(oversized.clone())
        .unwrap()
        .validate()
        .unwrap();
    for (message, reason) in [
        (oversized, "relay_request_too_large"),
        (json!({"text":""}), "empty_message"),
        (
            json!({"text":"\u{202a}\u{2069}\u{0000}\u{007f}"}),
            "empty_message",
        ),
    ] {
        let receipt = recording
            .reply(&channel, &event, message, ReplySource::Tool)
            .await;
        assert_eq!(
            receipt.delivery,
            DeliveryOutcome::Rejected {
                reason: reason.into()
            }
        );
        assert!(receipt.message_ref.is_none());
        assert!(recording.http.sent().is_empty());
    }
    let receipt = recording
        .reply(
            &channel,
            &event,
            json!({"text":"x".repeat(9000)}),
            ReplySource::Tool,
        )
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(recording.http.sent().len(), 1);
    assert_eq!(
        recording.http.sent()[0].header("authorization"),
        Some(format!("Bearer {REPLY_TOKEN}").as_str())
    );
    recording.finish().await;
}

#[tokio::test(start_paused = true)]
async fn partial_failure_lost_and_invalid_receipts_never_confirm_or_replay() {
    let bad_body = |error| Answer {
        reply: Ok(HttpReply {
            status: reqwest::StatusCode::OK,
            headers: Default::default(),
            body: Err(error),
        }),
        delay: Duration::ZERO,
    };
    let cases = [
        (
            Answer::status(429).retry_after("7"),
            "rate_limited_unconfirmed",
        ),
        (
            Answer::json(500, json!({"error":"part 2 failed after part 1"})),
            "provider_failure",
        ),
        (Answer::status(302), "provider_failure"),
        (Answer::status(200), "receipt_invalid"),
        (
            Answer::json(
                200,
                json!({"message_id":"inbound-1","platform_message_id":"first-part"}),
            ),
            "receipt_invalid",
        ),
        (
            Answer::json(200, json!({"message_id":"out-1","platform_message_id":42})),
            "receipt_invalid",
        ),
        (bad_body(BodyError::Lost), "response_lost"),
        (bad_body(BodyError::Oversized), "receipt_invalid"),
        (
            Answer {
                reply: Err(TransportError::Unavailable),
                delay: Duration::ZERO,
            },
            "response_lost",
        ),
        (
            Answer::status(200).after(Duration::from_secs(60)),
            "response_lost",
        ),
    ];
    for (answer, reason) in cases {
        let (mut recording, channel, event) = setup(FakeHttp::relay().script([answer]));
        let turn = turn(&recording.keys, &channel, relay_activity(), &event);
        let receipt = recording
            .reply(
                &channel,
                &event,
                json!({"text":"中文🦀".repeat(2000)}),
                ReplySource::Tool,
            )
            .await;
        assert_eq!(
            receipt.delivery,
            DeliveryOutcome::Unknown {
                reason: reason.into()
            }
        );
        assert!(receipt.message_ref.is_none());
        recording
            .outcome(
                &turn,
                TurnOutcome::Completed {
                    final_text: Some("do not duplicate".into()),
                },
            )
            .await;
        recording
            .reply(
                &channel,
                &event,
                json!({"text":"new intentional reply"}),
                ReplySource::Tool,
            )
            .await;
        let sent = recording.http.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            sent[1].header("authorization"),
            Some(format!("Bearer {CHANNEL_KEY}").as_str())
        );
        if reason == "rate_limited_unconfirmed" {
            assert!(sent[1].at - sent[0].at >= Duration::from_secs(7));
        }
        assert_eq!(
            recording.finish().await["calls"].as_array().unwrap().len(),
            2
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_bare_429_for_short_text_is_also_uncertain_in_complete_mode() {
    let (mut recording, channel, event) = setup(FakeHttp::relay().script([Answer::status(429)]));
    let receipt = recording
        .reply(&channel, &event, json!({"text":"hi"}), ReplySource::Tool)
        .await;
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Unknown {
            reason: "rate_limited_unconfirmed".into()
        }
    );
    assert_eq!(
        recording.finish().await["calls"].as_array().unwrap().len(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn compatible_logical_receipt_without_platform_id_is_confirmed() {
    let (mut recording, channel, event) = setup(FakeHttp::relay().script([Answer::json(
        200,
        json!({"message_id":"out-1","delivered_as":["text"]}),
    )]));
    let receipt = recording
        .reply(
            &channel,
            &event,
            json!({"text":"x".repeat(9000)}),
            ReplySource::Tool,
        )
        .await;
    assert_eq!(
        receipt.delivery,
        DeliveryOutcome::Confirmed {
            platform_message_id: None
        }
    );
    assert!(receipt.message_ref.is_none());
    assert_eq!(receipt.rendering.outcome, RenderingOutcome::Rendered);
    assert!(receipt.rendering.blocks.is_empty());
    recording.finish().await;
}

#[tokio::test(start_paused = true)]
async fn complete_mode_preserves_native_telegram_chunking() {
    let mut recording = start(FakeHttp::platforms(), DeliveryConfig::default());
    let channel = native_channel();
    let event = event(
        &channel,
        &relay_activity(),
        1,
        ReplyAddress::Telegram {
            bot_id: "77".into(),
            chat_id: "42".into(),
            message_id: "5".into(),
            thread_id: None,
        },
    );
    let receipt = recording
        .reply(
            &channel,
            &event,
            json!({"text":"🦀".repeat(4500)}),
            ReplySource::Tool,
        )
        .await;
    assert!(matches!(
        receipt.delivery,
        DeliveryOutcome::Confirmed { .. }
    ));
    assert_eq!(
        serde_json::to_value(receipt.rendering).unwrap()["blocks"][0]["outcome"]["how"],
        "chunked:3"
    );
    let sent = recording.http.sent();
    assert_eq!(sent.len(), 3);
    for request in &sent {
        assert!(request.url.path().ends_with("/sendMessage"));
        assert!(
            request.json()["text"]
                .as_str()
                .unwrap()
                .encode_utf16()
                .count()
                <= 4000
        );
    }
    recording.finish().await;
}

#[tokio::test(start_paused = true)]
async fn ack_progress_and_intentional_replies_keep_their_own_requests() {
    let recording = start(
        FakeHttp::relay(),
        DeliveryConfig {
            heartbeat: Duration::from_secs(10),
            ..Default::default()
        },
    );
    let channel = relay_channel(1000);
    let event = event(&channel, &relay_activity(), 1, relay_reply());
    let turn = turn(&recording.keys, &channel, relay_activity(), &event);
    recording
        .outcome(
            &turn,
            TurnOutcome::Accepted {
                response_id: "response-1".into(),
                thread_reply: false,
            },
        )
        .await;
    recording.at(1500).await;
    recording.at(12000).await;
    recording
        .outcome(
            &turn,
            TurnOutcome::MessageCommitted {
                text: "x".repeat(9000),
            },
        )
        .await;
    recording
        .outcome(
            &turn,
            TurnOutcome::MessageCommitted {
                text: "y".repeat(9000),
            },
        )
        .await;
    recording
        .outcome(
            &turn,
            TurnOutcome::Completed {
                final_text: Some("do not duplicate".into()),
            },
        )
        .await;
    let result = recording.finish().await;
    let texts: Vec<_> = result["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| call["body"]["reply"]["text"].as_str().unwrap())
        .collect();
    assert_eq!(
        texts,
        [
            "Working on it…",
            "Still working…",
            &"x".repeat(9000),
            &"y".repeat(9000)
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn every_relay_platform_delegates_long_text_without_platform_thresholds() {
    for platform in [
        Platform::Telegram,
        Platform::Discord,
        Platform::Slack,
        Platform::Lark,
        Platform::Feishu,
        Platform::Whatsapp,
    ] {
        let (mut recording, channel, mut event) = setup(FakeHttp::relay());
        if let ReplyAddress::NyxidRelay {
            platform: value, ..
        } = &mut event.reply
        {
            *value = platform;
        }
        let receipt = recording
            .reply(
                &channel,
                &event,
                json!({"text":"中文🦀".repeat(3000)}),
                ReplySource::Tool,
            )
            .await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        assert_eq!(receipt.rendering.outcome, RenderingOutcome::Rendered);
        assert_eq!(recording.http.sent().len(), 1);
        assert_eq!(
            recording.http.sent()[0].json()["reply"]["text"],
            "中文🦀".repeat(3000)
        );
        recording.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_closed_tool_receipt_cancels_before_dispatch_and_preserves_the_token() {
    use cmaeg::delivery::intents::DeliveryCommand;
    use tokio::sync::oneshot;
    let (mut recording, channel, event) = setup(FakeHttp::relay());
    let (receipt, received) = oneshot::channel();
    drop(received);
    recording
        .commands
        .send(DeliveryCommand::Reply {
            trace: tracing::Span::none(),
            channel: channel.clone(),
            event: event.clone(),
            message: serde_json::from_value(json!({"text":"cancelled"})).unwrap(),
            source: ReplySource::Tool,
            receipt,
        })
        .await
        .unwrap();
    let result = recording
        .reply(&channel, &event, json!({"text":"valid"}), ReplySource::Tool)
        .await;
    assert!(matches!(result.delivery, DeliveryOutcome::Confirmed { .. }));
    assert_eq!(recording.http.sent().len(), 1);
    assert_eq!(
        recording.http.sent()[0].header("authorization"),
        Some(format!("Bearer {REPLY_TOKEN}").as_str())
    );
    recording.finish().await;
}
