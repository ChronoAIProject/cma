//! CommonMark travels verbatim through the real sender and explicit NyxID routes.
use super::{
    fake_http::{Answer, FakeHttp},
    recording_fixtures::{event, relay_activity, relay_channel, relay_reply},
    recording_support::{Hooks, Recording},
};
use cmaeg::{
    delivery::intents::{DeliveryOutcome, ReplySource},
    net::exchange::TransportError,
    DeliveryConfig,
};
use cmaeg_core::{channel::Platform, seal::ReplyAddress};
use serde_json::json;
use std::time::Duration;

pub const SOURCE: &str = r#"BEGIN-QA-FMT
**中文标题**
- 食材：米100克。
**English title**
- Ingredient: rice 100 g.
**မြန်မာခေါင်းစဉ်**
- ပါဝင်ပစ္စည်း — ဆန် ၁၀၀ ဂရမ်။
[D1-B-ZH]
`C:\qa\menu_01.txt` 🦀 é
```text
C:\qa\menu_01.txt
```
END-QA-FMT"#;

fn recording(http: FakeHttp, enabled: bool) -> Recording {
    Recording::start_with_relay(
        http,
        Hooks::answering([]),
        DeliveryConfig::default(),
        None,
        true,
        |relay| {
            relay
                .with_commonmark_relay(enabled)
                .with_native_media(true)
                .with_text_edits(true)
        },
    )
}

#[tokio::test(start_paused = true)]
async fn commonmark_long_ordinary_and_thread_replies_reach_nyxid_once_verbatim() {
    let source = SOURCE.repeat(60);
    assert!(source.chars().count() > 11_000);
    for thread in [false, true] {
        let channel = relay_channel(0);
        let mut reply = relay_reply();
        if let ReplyAddress::NyxidRelay { thread_reply, .. } = &mut reply {
            *thread_reply = thread;
        }
        let event = event(&channel, &relay_activity(), 1, reply);
        let mut rec = recording(FakeHttp::relay(), true);
        let receipt = rec
            .reply(
                &channel,
                &event,
                json!({"text":source,"thread_reply":thread}),
                ReplySource::Tool,
            )
            .await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "{:?}",
            receipt.delivery
        );
        let sent = rec.http.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].url.path(),
            if thread {
                "/api/v1/channel-relay/thread-reply/commonmark"
            } else {
                "/api/v1/channel-relay/reply/commonmark"
            }
        );
        assert_eq!(sent[0].json()["reply"]["text"], source);
        assert_eq!(sent[0].json()["message_id"], "inbound-1");
        assert_eq!(
            sent[0].json().get("thread_reply"),
            thread.then_some(&json!(true))
        );
        assert_eq!(sent[0].timeout, Duration::from_secs(15));
        rec.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn commonmark_media_and_captions_keep_explicit_thread_route_and_attachment_metadata() {
    for thread in [false, true] {
        let channel = relay_channel(0);
        let mut reply = relay_reply();
        if let ReplyAddress::NyxidRelay { thread_reply, .. } = &mut reply {
            *thread_reply = thread;
        }
        let event = event(&channel, &relay_activity(), 1, reply);
        let mut rec = recording(FakeHttp::relay(), true);
        let attachments = json!([{"kind":"image","source":{"type":"base64","data":"iVBORw0KGgo="},"filename":"plot.png","mime_type":"image/png","caption":SOURCE.replace('\n', " ")}]);
        let receipt = rec
            .reply(
                &channel,
                &event,
                json!({"text":SOURCE,"thread_reply":thread,"native_attachments":attachments}),
                ReplySource::Tool,
            )
            .await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "{:?}",
            receipt.delivery
        );
        let sent = rec.http.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].url.path(),
            if thread {
                "/api/v1/channel-relay/thread-reply/commonmark"
            } else {
                "/api/v1/channel-relay/reply/commonmark"
            }
        );
        assert_eq!(sent[0].json()["reply"]["text"], SOURCE);
        assert_eq!(sent[0].json()["reply"]["attachments"], attachments);
        assert_eq!(sent[0].timeout, Duration::from_secs(60));
        rec.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn commonmark_nonconfirmation_never_replays_or_falls_back_to_legacy() {
    let answers = [
        Answer::status(400),
        Answer::status(404),
        Answer::status(429),
        Answer::status(502),
        Answer {
            reply: Err(TransportError::Unavailable),
            delay: Duration::ZERO,
        },
        Answer::json(
            200,
            json!({"message_id":"out-1","platform_message_id":"p-1"}),
        )
        .after(Duration::from_secs(16)),
    ];
    for answer in answers {
        for media in [false, true] {
            let channel = relay_channel(0);
            let event = event(&channel, &relay_activity(), 1, relay_reply());
            let mut rec = recording(FakeHttp::relay().script([answer.clone()]), true);
            let mut message = json!({"text":SOURCE.repeat(60)});
            if media {
                message["native_attachments"] =
                    json!([{"kind":"image","source":{"type":"base64","data":"iVBORw0KGgo="}}]);
            }
            let receipt = rec
                .reply(&channel, &event, message, ReplySource::Tool)
                .await;
            // Media has a 60 s exchange; the ordinary 16 s case exceeds only its 15 s limit.
            if media && answer.delay == Duration::from_secs(16) {
                assert!(matches!(
                    receipt.delivery,
                    DeliveryOutcome::Confirmed { .. }
                ));
            } else {
                assert!(
                    matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }),
                    "{:?}",
                    receipt.delivery
                );
                assert!(receipt.message_ref.is_none());
            }
            assert_eq!(rec.http.sent().len(), 1);
            assert!(rec.http.sent()[0].url.path().ends_with("/commonmark"));
            rec.finish().await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn commonmark_off_and_other_relay_platforms_keep_legacy_rendering_and_routes() {
    for (enabled, platform) in [
        (false, Platform::Telegram),
        (true, Platform::Discord),
        (true, Platform::Lark),
        (true, Platform::Whatsapp),
    ] {
        let channel = relay_channel(0);
        let mut reply = relay_reply();
        if let ReplyAddress::NyxidRelay {
            platform: value, ..
        } = &mut reply
        {
            *value = platform;
        }
        let event = event(&channel, &relay_activity(), 1, reply);
        let mut rec = recording(FakeHttp::relay(), enabled);
        let receipt = rec
            .reply(
                &channel,
                &event,
                json!({"text":"**bold** `path_1`"}),
                ReplySource::Tool,
            )
            .await;
        assert!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            "{:?}",
            receipt.delivery
        );
        let sent = rec.http.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].url.path(), "/api/v1/channel-relay/reply");
        // Lark shows plain text and Discord parses its own Markdown, so their
        // legacy route keeps the source punctuation; others keep the escapes.
        let expected = if matches!(platform, Platform::Discord | Platform::Lark) {
            "**bold** `path_1`"
        } else {
            r"\*\*bold\*\* \`path\_1\`"
        };
        assert_eq!(sent[0].json()["reply"]["text"], expected);
        rec.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn commonmark_plain_updates_keep_source_and_require_exact_edit_receipt() {
    for valid in [true, false] {
        let http = FakeHttp::new(move |sent, count| {
            if sent.url.path().contains("/update/") {
                Answer::json(
                    200,
                    json!({"upstream_message_id": if valid { "platform-1" } else { "wrong" },"edited_at":"2026-11-13T00:00:00Z"}),
                )
            } else {
                Answer::json(
                    200,
                    json!({"message_id":format!("out-{count}"),"platform_message_id":format!("platform-{count}")}),
                )
            }
        });
        let channel = relay_channel(0);
        let event = event(&channel, &relay_activity(), 1, relay_reply());
        let mut rec = recording(http, true);
        let original = rec
            .reply(&channel, &event, json!({"text":SOURCE}), ReplySource::Tool)
            .await;
        let reference = original
            .message_ref
            .unwrap()
            .open(&rec.keys)
            .unwrap()
            .payload;
        // Source exceeds the platform budget, while the repeated link text fits.
        let replacement = "[x](https://example.com/a/long/path) ".repeat(160);
        assert!(replacement.len() > 4000);
        let receipt = rec
            .update(&channel, reference, json!({"text":replacement}))
            .await;
        assert_eq!(
            matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }),
            valid
        );
        if !valid {
            assert!(matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }));
        }
        let sent = rec.http.sent();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            sent[1].url.path(),
            "/api/v1/channel-relay/reply/update/commonmark"
        );
        assert_eq!(sent[1].json()["reply"]["text"], replacement);
        assert_eq!(sent[1].json()["message_id"], "platform-1");
        assert_eq!(sent[1].json()["outbound_message_id"], "out-1");
        rec.finish().await;
    }
}

#[tokio::test(start_paused = true)]
async fn commonmark_switch_preserves_native_telegram_payloads() {
    use super::recording_fixtures::{activity, native_channel};
    use cmaeg_core::activity::SourceKind;
    let mut payloads = Vec::new();
    for enabled in [false, true] {
        let channel = native_channel();
        let activity = activity(
            SourceKind::Telegram {
                platform: Platform::Telegram,
                bot_id: "77".into(),
            },
            "native-commonmark",
        );
        let event = event(
            &channel,
            &activity,
            1,
            ReplyAddress::Telegram {
                bot_id: "77".into(),
                chat_id: "42".into(),
                message_id: "5".into(),
                thread_id: None,
            },
        );
        let mut rec = recording(FakeHttp::platforms(), enabled);
        let receipt = rec
            .reply(&channel, &event, json!({"text":SOURCE}), ReplySource::Tool)
            .await;
        assert!(matches!(
            receipt.delivery,
            DeliveryOutcome::Confirmed { .. }
        ));
        let sent = rec.http.sent();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].url.path().ends_with("/sendMessage"));
        payloads.push(sent[0].json());
        rec.finish().await;
    }
    assert_eq!(payloads[0], payloads[1]);
}
