use super::{native_lark_thread_start_fixture::*, *};
use crate::interactions::native_lark::fixture::Fixture;
use cmaeg_core::{activity::SourceKind, channel::LarkRegion};
use serde_json::Value;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn native_lark_thread_start_confirms_exact_root_thread_and_consumer_in_each_region() {
    for region in [LarkRegion::Lark, LarkRegion::Feishu] {
        for named in [false, true] {
            for existing in [false, true] {
                let f = Fixture::new_group(named, region).await;
                let (sender, api) = sender(&f);
                if existing {
                    api.anchor.lock().unwrap()["thread_id"] = "omt_started".into();
                }
                let receipt = send(&sender, &f, &request()).await;
                assert_eq!(receipt.delivery.kind(), "confirmed");
                assert_eq!(api.writes(), 1);
                let payload = receipt
                    .message_ref
                    .unwrap()
                    .open(&sender.keys)
                    .unwrap()
                    .payload;
                let a = payload.native_lark_effect.unwrap();
                assert_eq!(a.reply.thread_id.as_deref(), Some("omt_started"));
                assert_eq!(a.reply.message_id, "om_thread_reply");
                assert_eq!(payload.consumer, f.event.consumer);
                let stored = sender
                    .relay
                    .native_lark_receipts
                    .as_ref()
                    .unwrap()
                    .load(
                        &f.channel.id,
                        &SourceKind::NativeLark(a.reply.identity.clone()),
                        "om_thread_reply",
                        crate::interactions::native_lark::fixture::now(),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(stored.conversation_id, f.event.conversation_id);
                assert_eq!(stored.partition_key, f.event.partition_key);
                assert_eq!(
                    stored.consumer,
                    f.channel
                        .consumer(f.event.consumer.as_ref())
                        .unwrap()
                        .id
                        .clone()
                );
                let calls = api.calls.lock().unwrap();
                let (_, url, body) = calls
                    .iter()
                    .find(|(m, u, _)| *m == reqwest::Method::POST && u.path().ends_with("/reply"))
                    .unwrap();
                assert_eq!(url.path(), "/open-apis/im/v1/messages/om_original/reply");
                assert_eq!(body["reply_in_thread"], true);
                assert!(body.get("receive_id").is_none());
                assert!(calls.iter().all(|(_, u, _)| u.host_str()
                    == Some(if region == LarkRegion::Lark {
                        "open.larksuite.com"
                    } else {
                        "open.feishu.cn"
                    })));
            }
        }
    }
}

#[tokio::test]
async fn native_lark_thread_start_refuses_wrong_anchor_before_any_write() {
    for field in [
        "chat_id",
        "message_id",
        "create_time",
        "deleted",
        "root_id",
        "parent_id",
        "thread_id",
        "sender.id",
        "sender.tenant_key",
        "sender.sender_type",
        "sender.id_type",
    ] {
        let f = Fixture::new_group(false, LarkRegion::Feishu).await;
        let (sender, api) = sender(&f);
        {
            let mut row = api.anchor.lock().unwrap();
            if let Some(field) = field.strip_prefix("sender.") {
                row["sender"][field] = "wrong".into();
            } else {
                row[field] = match field {
                    "deleted" => true.into(),
                    "root_id" | "parent_id" => "om_foreign".into(),
                    "thread_id" => json!(123),
                    _ => "wrong".into(),
                };
            }
        }
        assert_eq!(
            send(&sender, &f, &request()).await.delivery.kind(),
            "rejected",
            "{field}"
        );
        assert_eq!(api.writes(), 0, "{field}");
    }
}

#[tokio::test]
async fn native_lark_thread_start_never_confirms_wrong_or_uncertain_destination() {
    for field in [
        "chat_id",
        "message_id",
        "root_id",
        "parent_id",
        "thread_id",
        "msg_type",
        "deleted",
        "sender.id",
        "sender.tenant_key",
        "sender.sender_type",
        "sender.id_type",
        "known_thread",
        "transport",
        "storage",
    ] {
        let f = Fixture::new_group(true, LarkRegion::Lark).await;
        let (sender, api) = sender(&f);
        if field == "transport" {
            api.fail_send.store(true, Ordering::SeqCst);
        } else if field == "storage" {
            let memory = f.server.memory.clone();
            use sha2::{Digest, Sha256};
            let hash = Sha256::digest(
                serde_json::to_vec(&(
                    "native_lark_receipt_v1",
                    &f.channel.id,
                    &f.context.activity.source,
                    "om_thread_reply",
                ))
                .unwrap(),
            );
            let mut bytes = [0; 16];
            bytes.copy_from_slice(&hash[..16]);
            bytes[6] = (bytes[6] & 15) | 64;
            bytes[8] = (bytes[8] & 63) | 128;
            let id = uuid::Uuid::from_bytes(bytes).to_string();
            *api.after_send.lock().unwrap() = Some(Box::new(move || {
                *memory.fault.lock().unwrap() =
                    Some(crate::ingress::routing_store::test_server::Fault {
                        id,
                        store: true,
                        lose_read: true,
                        skip: 0,
                    });
            }));
        } else if field == "known_thread" {
            api.anchor.lock().unwrap()["thread_id"] = "omt_other".into();
        } else {
            let mut row = api.sent.lock().unwrap();
            if let Some(field) = field.strip_prefix("sender.") {
                row["sender"][field] = "wrong".into();
            } else {
                row[field] = match field {
                    "deleted" => true.into(),
                    "message_id" => "om_original".into(),
                    "thread_id" => Value::Null,
                    _ => "wrong".into(),
                };
            }
        }
        let sent = send(&sender, &f, &request()).await;
        assert_eq!(sent.delivery.kind(), "unknown", "{field}");
        assert!(sent.message_ref.is_none());
        assert_eq!(api.writes(), 1, "{field}");
    }
}

#[tokio::test]
async fn native_lark_thread_start_is_explicit_gated_root_text_only() {
    for case in [
        "disabled",
        "private",
        "existing",
        "source",
        "oversize",
        "blocks",
        "attachments",
        "interaction",
    ] {
        let mut f = if case == "private" {
            Fixture::new(false, false, LarkRegion::Lark).await
        } else if case == "existing" {
            Fixture::new(false, true, LarkRegion::Lark).await
        } else {
            Fixture::new_group(false, LarkRegion::Lark).await
        };
        let (mut sender, api) = sender(&f);
        let mut message = request();
        match case {
            "disabled" => sender.relay = sender.relay.with_native_lark_thread_start(false),
            "source" => Arc::make_mut(&mut f.channel).definition.sources.clear(),
            "oversize" => message.text = Some("x".repeat(4001)),
            "blocks" => {
                message.blocks = vec![cmaeg_core::message::Block::Text {
                    text: "extra".into(),
                }]
            }
            "attachments" => {
                message.native_attachments =
                    vec![crate::native_lark::outbound::tests::fixture::attachment(
                        "image/png",
                    )]
            }
            "interaction" => f.event.event_id = "interaction-event".parse().unwrap(),
            _ => {}
        }
        let sent = send(&sender, &f, &message).await;
        assert_ne!(sent.delivery.kind(), "confirmed", "{case}");
        // The pre-existing thread follows the unchanged ordinary native transport.
        assert_eq!(api.writes(), usize::from(case == "existing"), "{case}");
    }
}
