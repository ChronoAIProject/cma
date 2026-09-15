use super::*;
use crate::bridge::ingress::capabilities::tests::fixtures::Fixture;
use serde_json::json;

fn projection(fixture: &Fixture) -> Option<VerifiedPlatformRedelivery> {
    let body = fixture.bytes();
    let signed = fixture.signed().unwrap();
    observe(&signed.authenticated_payload(&body).unwrap()).unwrap()
}

fn platforms() -> [BridgePlatform; 5] {
    [
        BridgePlatform::Telegram,
        BridgePlatform::TelegramNew,
        BridgePlatform::Discord,
        BridgePlatform::Lark,
        BridgePlatform::Feishu,
    ]
}

#[test]
fn fresh_signed_nyx_ids_and_transport_metadata_preserve_provider_equivalence() {
    for platform in platforms() {
        let first = Fixture::new(platform);
        let original = projection(&first).unwrap();
        let mut later = first.clone();
        later.body["message_id"] = json!("nyx-redelivery");
        later.body["correlation_id"] = json!("signed-redelivery");
        later.body["sender"]["display_name"] = json!("Changed display name");
        later.body["timestamp"] = json!("2026-09-15T00:00:00Z");
        later.body["thread_id"] = json!("interaction:app:different-protected-token");
        let equivalent = projection(&later).unwrap();
        assert_eq!(original.key_sha256(), equivalent.key_sha256());
        assert_eq!(original.semantic_sha256(), equivalent.semantic_sha256());
        let signed = later.signed().unwrap();
        let write = signed
            .classify_capabilities(&later.bytes(), &NyxidCapabilities)
            .unwrap()
            .bind(later.binding())
            .unwrap();
        assert!(matches!(
            write.with_platform_redelivery(Some(original)),
            Err(CallbackError::ReplayConflict)
        ));
    }
}

#[test]
fn exact_platform_chat_and_bot_keep_provider_namespaces_separate() {
    let first = Fixture::new(BridgePlatform::Telegram);
    let original = projection(&first).unwrap();
    let other_platform = projection(&Fixture::new(BridgePlatform::TelegramNew)).unwrap();
    assert_ne!(original.key_sha256(), other_platform.key_sha256());
    for dimension in ["chat", "bot", "bridge"] {
        let mut changed = first.clone();
        match dimension {
            "chat" => {
                changed.body["conversation"]["platform_id"] = json!("-100999");
                changed.raw_mut()["message"]["chat"]["id"] = json!(-100999);
            }
            "bot" => changed.config_mut().connector.bot_id = "other-bot".into(),
            _ => changed.bridge.id = layer2_core::bridge::BridgeId("brg_other".into()),
        }
        assert_ne!(
            original.key_sha256(),
            projection(&changed).unwrap().key_sha256(),
            "{dimension}"
        );
    }
}

#[test]
fn changed_text_sender_topic_and_parent_conflict_under_one_provider_identity() {
    let first = Fixture::new(BridgePlatform::TelegramNew);
    let original = projection(&first).unwrap();
    for dimension in ["text", "sender", "thread", "parent", "conversation"] {
        let mut changed = first.clone();
        match dimension {
            "text" => {
                changed.body["content"]["text"] = json!("changed text");
                changed.raw_mut()["message"]["text"] = json!("changed text");
            }
            "sender" => {
                changed.body["sender"]["platform_id"] = json!("54321");
                changed.raw_mut()["message"]["from"]["id"] = json!(54321);
            }
            "thread" => {
                changed.raw_mut()["message"]["is_topic_message"] = json!(true);
                changed.raw_mut()["message"]["message_thread_id"] = json!(7);
            }
            "parent" => {
                changed.body["reply_to_platform_message_id"] = json!("41");
                changed.raw_mut()["message"]["reply_to_message"] =
                    json!({"message_id":41,"chat":{"id":-100123}});
            }
            _ => changed.raw_mut()["message"]["chat"]["type"] = json!("private"),
        }
        let result = projection(&changed).unwrap();
        assert_eq!(original.key_sha256(), result.key_sha256(), "{dimension}");
        assert_ne!(
            original.semantic_sha256(),
            result.semantic_sha256(),
            "{dimension}"
        );
    }
}

#[test]
fn missing_parent_proof_media_bots_edits_and_unsigned_replacements_create_no_alias_proof() {
    let first = Fixture::new(BridgePlatform::Telegram);
    for dimension in [
        "parent",
        "foreign_parent",
        "external_reply",
        "media",
        "bot",
        "edit",
        "generic",
    ] {
        let mut changed = first.clone();
        match dimension {
            "parent" => changed.body["reply_to_platform_message_id"] = json!("41"),
            "foreign_parent" => {
                changed.body["reply_to_platform_message_id"] = json!("41");
                changed.raw_mut()["message"]["reply_to_message"] =
                    json!({"message_id":41,"chat":{"id":-200123}});
            }
            "external_reply" => changed.raw_mut()["message"]["external_reply"] = json!({}),
            "media" => changed.raw_mut()["message"]["photo"] = json!([{"file_id":"private"}]),
            "bot" => changed.raw_mut()["message"]["from"]["is_bot"] = json!(true),
            "edit" => {
                let message = changed
                    .raw_mut()
                    .as_object_mut()
                    .unwrap()
                    .remove("message")
                    .unwrap();
                changed.raw_mut()["edited_message"] = message;
            }
            _ => {
                changed
                    .body
                    .as_object_mut()
                    .unwrap()
                    .remove("raw_platform_data");
            }
        }
        assert!(projection(&changed).is_none(), "{dimension}");
    }
    let signed = first.signed().unwrap();
    let mut replacement = first;
    replacement.body["message_id"] = json!("unsigned-substitution");
    assert!(matches!(
        signed.authenticated_payload(&replacement.bytes()),
        Err(CallbackError::Authentication)
    ));
}

#[test]
fn provider_parent_fields_require_exact_signed_corroboration_across_platforms() {
    for platform in platforms() {
        let base = Fixture::new(platform.clone());
        let original = projection(&base).unwrap();
        let mut reply = base;
        let parent = match platform {
            BridgePlatform::Telegram | BridgePlatform::TelegramNew => {
                reply.raw_mut()["message"]["reply_to_message"] =
                    json!({"message_id":41,"chat":{"id":-100123}});
                "41"
            }
            BridgePlatform::Discord => {
                reply.raw_mut()["d"]["type"] = json!(19);
                reply.raw_mut()["d"]["message_reference"] =
                    json!({"type":0,"channel_id":"discord-chat","message_id":"discord-parent"});
                "discord-parent"
            }
            BridgePlatform::Lark | BridgePlatform::Feishu => {
                reply.raw_mut()["event"]["message"]["parent_id"] = json!("om_parent");
                "om_parent"
            }
        };
        reply.body["reply_to_platform_message_id"] = json!(parent);
        let proven = projection(&reply).unwrap();
        assert_eq!(original.key_sha256(), proven.key_sha256());
        assert_ne!(original.semantic_sha256(), proven.semantic_sha256());
        reply.body["reply_to_platform_message_id"] = json!("unproven-parent");
        assert!(projection(&reply).is_none());
    }
}
