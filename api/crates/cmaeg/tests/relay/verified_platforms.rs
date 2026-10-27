//! Real signed callback → normalization/admission/launch contracts for opt-in sources.
use super::support::{Envelope, Fixture};
use cmaeg_core::activity::{Activity, ActivityKind, ActorKind, Attachment, AttachmentSource};
use serde_json::{json, Value};

fn verified(value: &mut Value) {
    value["sources"][0]["verify_raw_event"] = true.into();
    value["sources"][0]["bot_id"] = "9".into();
}

fn envelope(platform: &str) -> Envelope {
    let mut e = Envelope::on(platform);
    if platform == "discord" {
        e.payload["conversation"]["type"] = "group".into();
        e.payload["raw_platform_data"] = json!({"t":"MESSAGE_CREATE","d":{
            "id":"11","channel_id":"42","author":{"id":"7","bot":false},"content":"hello","mentions":[],"type":0
        }});
    } else {
        e.payload["conversation"]["platform_id"] = "7".into();
        e.payload["raw_platform_data"] =
            json!({"id":"wamid.message1","from":"7","type":"text","text":{"body":"hello"}});
    }
    e
}

async fn accepted(f: &Fixture, e: &Envelope) -> Activity {
    let response = f.send(e).await;
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["status"], "accepted", "{body}");
    f.recorder
        .requests
        .lock()
        .unwrap()
        .pop()
        .unwrap()
        .activity
        .clone()
}

async fn refused(f: &Fixture, e: &Envelope, reason: &str) {
    let response = f.send(e).await;
    assert_eq!(response.status(), 202);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body, json!({"status":"refused","reason":reason}));
    assert!(f.recorder.requests.lock().unwrap().is_empty());
}

fn attachments(activity: &Activity) -> &[Attachment] {
    let ActivityKind::Message { attachments, .. } = &activity.kind else {
        panic!("expected message");
    };
    attachments
}

fn media(platform: &str, kind: &str) -> Envelope {
    let mut e = envelope(platform);
    e.payload["content"] = json!({"type":kind,"text":"caption","attachments":[]});
    let mime = match kind {
        "image" => "image/png",
        "audio" => "audio/ogg",
        "video" => "video/mp4",
        _ => "application/pdf",
    };
    let (message_id, url) = if platform == "discord" {
        let message = &mut e.payload["raw_platform_data"]["d"];
        message["content"] = "caption".into();
        message["attachments"] = json!([{"id":"99","url":"https://cdn.discordapp.com/a","content_type":mime,"filename":"media.bin","size":10}]);
        ("11", "https://cdn.discordapp.com/a")
    } else {
        let native = if kind == "file" { "document" } else { kind };
        e.payload["raw_platform_data"] = json!({"id":"wamid.message1","from":"7","type":native,native:{"id":"99","mime_type":mime,"filename":"media.bin","caption":"caption"}});
        ("wamid.message1", "https://graph.facebook.com/v23.0/99")
    };
    e.payload["content"]["attachments"] = json!([{"content_type":kind,"url":url,"platform_message_id":message_id,"file_key":"99","filename":"media.bin","mime_type":mime}]);
    e
}

#[tokio::test]
async fn opt_in_binds_raw_sender_chat_and_text_without_changing_legacy_trust() {
    for platform in ["discord", "whatsapp"] {
        let f = Fixture::on(platform, verified, Default::default()).await;
        let e = envelope(platform);
        let activity = accepted(&f, &e).await;
        assert_eq!(
            activity.actor.kind,
            if platform == "discord" {
                ActorKind::Human
            } else {
                ActorKind::Unknown
            }
        );
        for pointer in [
            "/sender/platform_id",
            "/conversation/platform_id",
            "/conversation/type",
        ] {
            let mut bad = envelope(platform);
            *bad.payload.pointer_mut(pointer).unwrap() = "different".into();
            assert_eq!(f.send(&bad).await.status(), 401, "{platform} {pointer}");
        }
        let mut bad = envelope(platform);
        bad.payload["content"]["text"] = "spoofed".into();
        refused(&f, &bad, "content_unsupported").await;
        bad.payload["raw_platform_data"] = Value::Null;
        refused(&f, &bad, "unknown_sender").await;

        let old = Fixture::on_with_media(
            platform,
            |v| v["sources"][0]["trust_normalized"] = true.into(),
            Default::default(),
        )
        .await;
        let mut rawless = e;
        let original_raw = rawless.payload["raw_platform_data"].clone();
        rawless
            .payload
            .as_object_mut()
            .unwrap()
            .remove("raw_platform_data");
        let before = accepted(&old, &rawless).await;
        rawless.payload["raw_platform_data"] = original_raw;
        let after = accepted(&old, &rawless).await;
        assert_eq!(before, after);
        assert_eq!(after.actor.kind, ActorKind::Unknown);
        refused(&old, &media(platform, "image"), "attachment_unsupported").await;
    }
}

#[tokio::test]
async fn every_verified_media_kind_is_bound_to_the_original_download_index() {
    for platform in ["discord", "whatsapp"] {
        let f = Fixture::on_with_media(platform, verified, Default::default()).await;
        for kind in ["image", "audio", "video", "file"] {
            let mut e = media(platform, kind);
            // A dropped unrecognized item must never renumber a usable locator.
            e.payload["content"]["attachments"]
                .as_array_mut()
                .unwrap()
                .insert(0, json!(12));
            let activity = accepted(&f, &e).await;
            let items = attachments(&activity);
            assert_eq!(items.len(), 1);
            assert_eq!(
                items[0].source,
                Some(AttachmentSource::NyxidRelay { index: 1 })
            );
            assert_eq!(serde_json::to_value(items[0].kind).unwrap(), kind);
            assert_eq!(items[0].reference, format!("{platform}:99"));
            activity.validate().unwrap();
        }
    }
}

#[tokio::test]
async fn media_never_uses_another_file_or_native_message_from_the_envelope() {
    for platform in ["discord", "whatsapp"] {
        let f = Fixture::on_with_media(platform, verified, Default::default()).await;
        for (field, value) in [
            ("file_key", "100"),
            ("platform_message_id", "other-message"),
            ("content_type", "audio"),
        ] {
            let mut e = media(platform, "image");
            e.payload["content"]["attachments"][0][field] = value.into();
            let activity = accepted(&f, &e).await;
            assert!(
                attachments(&activity)[0].source.is_none(),
                "{platform} {field}"
            );
        }
        let mut e = envelope(platform);
        e.payload["content"]["attachments"] =
            media(platform, "image").payload["content"]["attachments"].clone();
        refused(&f, &e, "attachment_unsupported").await;
    }
    let f = Fixture::on_with_media("discord", verified, Default::default()).await;
    let mut e = media("discord", "image");
    e.payload["content"]["attachments"][0]["url"] = "https://cdn.discordapp.com/different".into();
    assert!(attachments(&accepted(&f, &e).await)[0].source.is_none());
}

#[tokio::test]
async fn verified_media_switch_and_sender_fact_repairs_preserve_messages() {
    for platform in ["discord", "whatsapp"] {
        let off = Fixture::on(platform, verified, Default::default()).await;
        refused(&off, &media(platform, "image"), "attachment_unsupported").await;
        let on = Fixture::on_with_media(platform, verified, Default::default()).await;
        let mut e = media(platform, "file");
        let raw = if platform == "discord" {
            &mut e.payload["raw_platform_data"]["d"]["attachments"][0]
        } else {
            &mut e.payload["raw_platform_data"]["document"]
        };
        raw["filename"] = format!("{}\u{0001}", "名".repeat(200)).into();
        raw["size"] = json!(u64::MAX);
        let activity = accepted(&on, &e).await;
        activity.validate().unwrap();
        assert!(attachments(&activity)[0]
            .filename
            .as_ref()
            .is_none_or(|name| name.len() <= 255));
        assert!(attachments(&activity)[0].source.is_some());
    }
}

#[tokio::test]
async fn discord_refuses_bot_system_edit_and_forged_parent_events() {
    let f = Fixture::on("discord", verified, Default::default()).await;
    for (pointer, value, reason) in [
        ("/raw_platform_data/d/author/bot", json!(true), "bot_sender"),
        ("/raw_platform_data/d/type", json!(7), "system_event"),
        (
            "/raw_platform_data/t",
            json!("MESSAGE_UPDATE"),
            "message_edit",
        ),
        (
            "/raw_platform_data/t",
            json!("MESSAGE_DELETE"),
            "system_event",
        ),
    ] {
        let mut e = envelope("discord");
        *e.payload.pointer_mut(pointer).unwrap() = value;
        refused(&f, &e, reason).await;
    }
    for (key, value, reason) in [
        ("system", json!(true), "system_sender"),
        ("id", json!("9"), "bot_sender"),
    ] {
        let mut e = envelope("discord");
        e.payload["raw_platform_data"]["d"]["author"][key] = value.clone();
        if key == "id" {
            e.payload["sender"]["platform_id"] = value;
        }
        refused(&f, &e, reason).await;
    }
    let mut e = envelope("discord");
    e.payload["raw_platform_data"]["d"]["webhook_id"] = "10".into();
    refused(&f, &e, "bot_sender").await;
    let mut e = envelope("discord");
    e.payload["raw_platform_data"]["d"]["author"]["bot"] = "false".into();
    assert_eq!(f.send(&e).await.status(), 401);
    let mut e = envelope("discord");
    e.payload["reply_to_platform_message_id"] = "12".into();
    e.payload["raw_platform_data"]["d"]["message_reference"] = json!({"message_id":"12"});
    e.payload["raw_platform_data"]["d"]["referenced_message"] =
        json!({"id":"13","author":{"id":"9"}});
    assert_eq!(f.send(&e).await.status(), 401);
}

#[tokio::test]
async fn discord_group_admission_requires_the_pinned_bot_mention_or_parent() {
    let f = Fixture::on("discord", |v| {
        verified(v);
        v["sources"][0]["admission"] = json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"open"},"groups":"mention_or_reply_to_bot"});
    }, Default::default()).await;
    let mut e = envelope("discord");
    refused(&f, &e, "group_not_addressed").await;
    e.payload["raw_platform_data"]["d"]["mentions"] = json!([{"id":"8"}]);
    refused(&f, &e, "group_not_addressed").await;
    e.payload["raw_platform_data"]["d"]["mentions"] = json!([{"id":"9"}]);
    accepted(&f, &e).await;
    let mut e = envelope("discord");
    e.payload["reply_to_platform_message_id"] = "12".into();
    e.payload["raw_platform_data"]["d"]["message_reference"] =
        json!({"message_id":"12","channel_id":"42"});
    refused(&f, &e, "addressing_evidence_unavailable").await;
    e.payload["raw_platform_data"]["d"]["referenced_message"] =
        json!({"id":"12","channel_id":"42","author":{"id":"9"}});
    accepted(&f, &e).await;
    e.payload["raw_platform_data"]["d"]["referenced_message"]["channel_id"] = "43".into();
    assert_eq!(f.send(&e).await.status(), 401);
}

#[tokio::test]
async fn discord_interaction_credentials_never_become_threads_or_trusted_actions() {
    let f = Fixture::on_with_media("discord", verified, Default::default()).await;
    let mut e = envelope("discord");
    e.payload["content"]["text"] = "/ask prompt=help".into();
    e.payload["thread_id"] = "interaction:9:opaque-followup".into();
    e.payload["raw_platform_data"] = json!({"id":"11","channel_id":"42","application_id":"9","token":"opaque-followup","type":2,"member":{"user":{"id":"7"}},"data":{"name":"ask","options":[{"name":"prompt","value":"help"}]}});
    let activity = accepted(&f, &e).await;
    assert!(activity.conversation.thread_id.is_none());
    assert!(activity.thread.is_none());
    assert!(!serde_json::to_string(&activity)
        .unwrap()
        .contains("opaque-followup"));
    e.payload["raw_platform_data"]["type"] = 4.into();
    refused(&f, &e, "system_event").await;
    e.payload["raw_platform_data"]["type"] = 3.into();
    e.payload["raw_platform_data"]["data"] = json!({"custom_id":"choose-one"});
    e.payload["content"]["text"] = "choose-one".into();
    assert!(matches!(
        accepted(&f, &e).await.kind,
        ActivityKind::Message { .. }
    ));
    e.payload["raw_platform_data"]["application_id"] = "8".into();
    assert_eq!(f.send(&e).await.status(), 401);
}

#[tokio::test]
async fn whatsapp_controls_and_locations_are_content_not_consent_and_receipts_start_no_turn() {
    let f = Fixture::on_with_media("whatsapp", verified, Default::default()).await;
    for (kind, raw, text) in [
        (
            "button",
            json!({"text":"Confirm","payload":"private-id"}),
            "Confirm",
        ),
        (
            "interactive",
            json!({"list_reply":{"id":"first","title":"First"}}),
            "First",
        ),
        (
            "location",
            json!({"latitude":1.5,"longitude":2.5,"name":"Place"}),
            "Location: 1.5, 2.5; Place",
        ),
        (
            "contacts",
            json!([{"name":{"formatted_name":"Alice"}}]),
            "Contacts: Alice",
        ),
    ] {
        let mut e = envelope("whatsapp");
        e.payload["raw_platform_data"] =
            json!({"id":"wamid.message1","from":"7","type":kind,kind:raw});
        e.payload["content"]["text"] = text.into();
        let activity = accepted(&f, &e).await;
        assert!(matches!(activity.kind, ActivityKind::Message { .. }));
    }
    for kind in ["reaction", "statuses", "protocol", "order"] {
        let mut e = envelope("whatsapp");
        e.payload["raw_platform_data"]["type"] = kind.into();
        refused(&f, &e, "system_event").await;
    }
}

#[tokio::test]
async fn discord_thread_partition_uses_only_explicit_membership_and_not_child_threads() {
    let f = Fixture::on(
        "discord",
        |v| {
            verified(v);
            v["partition"]["key"] = "thread".into();
        },
        Default::default(),
    )
    .await;
    let mut e = envelope("discord");
    e.payload["thread_id"] = "unverified-thread".into();
    e.payload["thread_context"] = json!({"native_thread_id":"unverified-thread"});
    e.payload["raw_platform_data"]["d"]["thread"] = json!({"id":"50","type":11});
    refused(&f, &e, "thread_evidence_unavailable").await;
    e.payload["raw_platform_data"]["d"]["channel"] = json!({"id":"42","type":11});
    let activity = accepted(&f, &e).await;
    assert_eq!(activity.conversation.thread_id.as_deref(), Some("42"));
    assert!(
        activity.thread.is_none(),
        "partition evidence is not relay thread reply authority"
    );
    e.payload["raw_platform_data"]["d"]["channel"]["id"] = "50".into();
    assert_eq!(f.send(&e).await.status(), 401);
}

#[tokio::test]
async fn discord_interaction_files_and_bounded_albums_use_matching_resource_handles() {
    let f = Fixture::on_with_media("discord", verified, Default::default()).await;
    let mut e = media("discord", "image");
    let file = e.payload["raw_platform_data"]["d"]["attachments"][0].clone();
    e.payload["raw_platform_data"] = json!({"id":"11","channel_id":"42","application_id":"9","type":2,"user":{"id":"7"},"channel":{"type":1},"data":{"name":"inspect","resolved":{"attachments":{"99":file}}}});
    e.payload["conversation"]["type"] = "private".into();
    e.payload["content"]["type"] = "text".into();
    e.payload["content"]["text"] = "/inspect".into();
    let activity = accepted(&f, &e).await;
    assert_eq!(
        attachments(&activity)[0].source,
        Some(AttachmentSource::NyxidRelay { index: 0 })
    );
    let mut e = media("discord", "image");
    let mut raw = Vec::new();
    let mut entries = Vec::new();
    for index in 0..20 {
        let id = (100 + index).to_string();
        raw.push(
            json!({"id":id,"url":"https://cdn.discordapp.com/image","content_type":"image/png"}),
        );
        entries.push(json!({"file_key":id,"url":"https://cdn.discordapp.com/image","platform_message_id":"11","content_type":"image"}));
    }
    e.payload["raw_platform_data"]["d"]["attachments"] = raw.into();
    e.payload["content"]["attachments"] = entries.into();
    let activity = accepted(&f, &e).await;
    assert_eq!(attachments(&activity).len(), 16);
    assert_eq!(
        attachments(&activity)[15].source,
        Some(AttachmentSource::NyxidRelay { index: 15 })
    );
    activity.validate().unwrap();
}

#[tokio::test]
async fn whatsapp_captionless_voice_and_stickers_keep_usable_media() {
    let f = Fixture::on_with_media("whatsapp", verified, Default::default()).await;
    for (kind, variant) in [("audio", "voice"), ("image", "sticker")] {
        let mut e = media("whatsapp", kind);
        e.payload["content"].as_object_mut().unwrap().remove("text");
        let mut raw_media = e.payload["raw_platform_data"][kind].take();
        raw_media.as_object_mut().unwrap().remove("caption");
        if variant == "voice" {
            raw_media["voice"] = true.into();
        }
        let raw_type = if variant == "sticker" {
            "sticker"
        } else {
            kind
        };
        e.payload["raw_platform_data"]["type"] = raw_type.into();
        e.payload["raw_platform_data"][raw_type] = raw_media;
        let activity = accepted(&f, &e).await;
        assert_eq!(
            serde_json::to_value(attachments(&activity)[0].variant).unwrap(),
            variant
        );
        assert!(attachments(&activity)[0].source.is_some());
    }
}
