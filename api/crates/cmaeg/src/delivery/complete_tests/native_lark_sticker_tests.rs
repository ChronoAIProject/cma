//! Exact retained stickers use no download/upload path or caller-selected key.
use super::{
    native_lark_thread_start_fixture::{send, sender, Api},
    *,
};
use crate::{
    interactions::native_lark::fixture::{now, Fixture},
    native_lark::{message, stickers, test_support},
};
use cmaeg_core::channel::{LarkRegion, Secret, Source};
use serde_json::Value;
use std::sync::atomic::Ordering;

async fn retained(f: &mut Fixture, tamper: bool) {
    let Source::NativeLark(source) = &f.channel.definition.sources[0] else {
        panic!()
    };
    let mut raw = test_support::text("om_sticker", "", now());
    raw["event"]["message"]["message_type"] = "sticker".into();
    raw["event"]["message"]["content"] = json!({"file_key":"retained-sticker-key"})
        .to_string()
        .into();
    if let Some(thread) = &f.context.activity.conversation.thread_id {
        raw["event"]["message"]["chat_type"] = "group".into();
        raw["event"]["message"]["thread_id"] = thread.clone().into();
    }
    let msg = message::extract(source, &raw, now()).unwrap().unwrap();
    let request = message::launch(
        f.channel.clone(),
        source,
        &msg,
        None,
        &crate::ingress::routing_plan::fixture::keys(),
        std::time::Instant::now(),
        now(),
    )
    .unwrap();
    let mut body = serde_json::to_value(msg).unwrap();
    if tamper {
        body["received_sticker"]["file_key"] = "mutated-sticker-key".into();
    }
    (f.context.activity, f.event) =
        crate::turns::routed::bind_received_message_fixture_with_source(
            f.server.records.clone(),
            Arc::new(crate::ingress::routing_plan::fixture::keys()),
            &request,
            f.event.consumer.as_ref(),
            &serde_json::to_vec(&body).unwrap(),
        )
        .await;
}
fn request() -> OutboundMessage {
    serde_json::from_value(json!({"reply_with_received_sticker":true})).unwrap()
}
fn writes(api: &Api) -> Vec<(reqwest::Method, Url, Value)> {
    api.calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(m, u, _)| {
            *m == reqwest::Method::POST && u.path().starts_with("/open-apis/im/v1/messages")
        })
        .cloned()
        .collect()
}
fn configure(sender: &mut Sender, api: &Api, f: &Fixture) {
    sender.relay = sender.relay.clone().with_native_lark_sticker_replies(true);
    let ReplyAddress::NativeLark(reply) = &f.event.reply else {
        panic!()
    };
    *api.sent.lock().unwrap() = json!({"message_id":"om_sticker_sent","chat_id":reply.chat_id,
        "thread_id":reply.thread_id.as_deref().unwrap_or("")});
}
#[tokio::test]
async fn received_sticker_sends_exact_key_to_original_chat_or_thread_in_each_region() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        for named in [false, true] {
            for threaded in [false, true] {
                let mut f = Fixture::new(named, threaded, region).await;
                retained(&mut f, false).await;
                let (mut sender, api) = sender(&f);
                configure(&mut sender, &api, &f);
                let result = send(&sender, &f, &request()).await;
                assert_eq!(result.delivery.kind(), "confirmed");
                let writes = writes(&api);
                assert_eq!(writes.len(), 1);
                let (_, url, body) = &writes[0];
                assert_eq!(body["msg_type"], "sticker");
                assert_eq!(
                    serde_json::from_str::<Value>(body["content"].as_str().unwrap()).unwrap(),
                    json!({"file_key":"retained-sticker-key"})
                );
                if threaded {
                    assert_eq!(url.path(), "/open-apis/im/v1/messages/om_sticker/reply");
                    assert_eq!(body["reply_in_thread"], true);
                    assert!(body.get("receive_id").is_none());
                } else {
                    assert_eq!(body["receive_id"], f.context.activity.conversation.id);
                }
                assert!(!api.calls.lock().unwrap().iter().any(|(_, u, _)| u
                    .path()
                    .contains("resources")
                    || u.path().ends_with("/files")
                    || u.path().ends_with("/images")));
                let receipt = sender
                    .relay
                    .native_lark_receipts
                    .as_ref()
                    .unwrap()
                    .load(
                        &f.channel.id,
                        &f.context.activity.source,
                        "om_sticker_sent",
                        now(),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(receipt.original.event_id, f.event.event_id);
                assert_eq!(
                    receipt.consumer,
                    f.channel
                        .consumer(f.event.consumer.as_ref())
                        .unwrap()
                        .id
                        .clone()
                );
            }
        }
    }
}
#[tokio::test]
async fn received_sticker_rejects_missing_custody_source_key_owner_or_consumer_changes_before_send()
{
    for defect in [
        "unretained",
        "key",
        "owner",
        "source",
        "credential",
        "consumer",
        "recipient",
        "gate",
    ] {
        let mut f = Fixture::new(true, true, LarkRegion::Feishu).await;
        if defect != "unretained" {
            retained(&mut f, defect == "key").await;
        }
        let (mut sender, api) = sender(&f);
        configure(&mut sender, &api, &f);
        match defect {
            "owner" => {
                Arc::make_mut(&mut f.channel).definition.owner.subject = "another-owner".into()
            }
            "source" => Arc::make_mut(&mut f.channel).definition.sources.clear(),
            "credential" => {
                Arc::make_mut(&mut f.channel)
                    .definition
                    .secrets
                    .native_lark
                    .as_mut()
                    .unwrap()
                    .app_secret = Secret::new("rotated")
            }
            "consumer" => Arc::make_mut(&mut f.channel)
                .definition
                .routing
                .as_mut()
                .unwrap()
                .consumers
                .clear(),
            "recipient" => {
                if let ReplyAddress::NativeLark(reply) = &mut f.event.reply {
                    reply.chat_id = "oc_foreign".into();
                }
            }
            "gate" => sender.relay = sender.relay.clone().with_native_lark_sticker_replies(false),
            _ => {}
        }
        let result = send(&sender, &f, &request()).await;
        assert_ne!(result.delivery.kind(), "confirmed", "{defect}");
        assert!(writes(&api).is_empty(), "{defect}");
    }
}
#[tokio::test]
async fn received_sticker_unknown_send_has_one_attempt_and_no_text_fallback() {
    let mut f = Fixture::new(false, true, LarkRegion::Lark).await;
    retained(&mut f, false).await;
    let (mut sender, api) = sender(&f);
    configure(&mut sender, &api, &f);
    api.fail_send.store(true, Ordering::SeqCst);
    let result = send(&sender, &f, &request()).await;
    assert_eq!(result.delivery.kind(), "unknown");
    assert_eq!(writes(&api).len(), 1);
}
#[test]
fn received_sticker_request_is_exclusive_and_old_serialization_stays_identical() {
    let old = json!({"text":"unchanged","blocks":[]});
    let message: OutboundMessage = serde_json::from_value(old.clone()).unwrap();
    use sha2::{Digest, Sha256};
    let legacy_bytes = br#"{"text":"unchanged","blocks":[]}"#;
    let bytes = serde_json::to_vec(&message).unwrap();
    assert_eq!(bytes, legacy_bytes);
    assert_eq!(Sha256::digest(&bytes), Sha256::digest(legacy_bytes));
    assert_eq!(serde_json::to_value(message).unwrap(), old);
    assert!(request().validate().is_ok());
    for extra in [
        json!({"text":"caption"}),
        json!({"thread_reply":true}),
        json!({"clear_native_attachments":true}),
        json!({"blocks":[{"type":"text","text":"caption"}]}),
        json!({"reply_to":"cmaeg1.msg.other"}),
    ] {
        let mut value = json!({"reply_with_received_sticker":true});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(
            !serde_json::from_value::<OutboundMessage>(value).is_ok_and(|m| m.validate().is_ok())
        );
    }
    assert!(serde_json::from_value::<OutboundMessage>(
        json!({"reply_with_received_sticker":true,"file_key":"arbitrary"})
    )
    .is_err());
    let sticker =
        stickers::ReceivedSticker::parse(&json!({"file_key":"private-sticker-key"})).unwrap();
    let attachment = sticker.attachment("om_private");
    assert!(attachment.source.is_none() && attachment.thumbnail.is_none());
    assert!(!serde_json::to_string(&attachment)
        .unwrap()
        .contains("private-sticker-key"));
    for value in [
        json!({"file_key":"https://untrusted.example"}),
        json!({"file_key":"valid","url":"https://example.org"}),
        json!({}),
    ] {
        assert!(stickers::ReceivedSticker::parse(&value).is_err());
    }
}

#[test]
fn ordinary_retained_native_source_bytes_and_hash_stay_identical() {
    use sha2::{Digest, Sha256};
    let Source::NativeLark(source) = test_support::source() else {
        panic!()
    };
    let raw = test_support::text("om_legacy", "unchanged", 1800000000);
    let parsed = message::extract(&source, &raw, 1800000000)
        .unwrap()
        .unwrap();
    let legacy = br#"{"kind":"text","message_id":"om_legacy","chat_id":"oc_native_chat","sender":"ou_human","text":"unchanged","group":false,"thread_id":null,"mentioned":false,"created_ms":1800000000000}"#;
    let current = serde_json::to_vec(&parsed).unwrap();
    assert_eq!(current, legacy);
    assert_eq!(Sha256::digest(&current), Sha256::digest(legacy));
}
