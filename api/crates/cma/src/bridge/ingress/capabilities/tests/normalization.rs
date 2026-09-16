use super::*;

#[test]
fn signed_platform_text_uses_actual_chat_type_and_retains_distinct_default_partitions() {
    for platform in platforms() {
        let fixture = Fixture::new(platform.clone());
        let write = fixture.write().unwrap();
        assert_eq!(write.record().admission, InboxAdmission::Accepted);
        assert_eq!(write.record().envelope.conversation_type, "group");
        let source =
            NormalizedPartitionSource::from_stored(write.record(), &fixture.bridge).unwrap();
        assert!(source.provider_message_id().is_some());
        assert!(matches!(
            source.admit(fixture.config()).unwrap(),
            PartitionAdmissionDecision::Admitted {
                reply_visibility: ReplyVisibility::OriginalGroupOrChannel,
                ..
            }
        ));
        let initial = fixture.key();
        let mut changed = fixture.clone();
        changed.body["message_id"] = json!("another-nyx-message");
        changed.body["sender"]["display_name"] = json!("Renamed person");
        assert_eq!(initial, changed.key());
        let mut sender = fixture.clone();
        let mut chat = fixture.clone();
        let mut private = fixture.clone();
        match platform {
            BridgePlatform::Telegram | BridgePlatform::TelegramNew => {
                sender.body["sender"]["platform_id"] = json!("67890");
                sender.raw_mut()["message"]["from"]["id"] = json!(67890);
                chat.body["conversation"]["platform_id"] = json!("-100456");
                chat.raw_mut()["message"]["chat"]["id"] = json!(-100456);
                private.raw_mut()["message"]["chat"]["type"] = json!("private");
            }
            BridgePlatform::Lark | BridgePlatform::Feishu => {
                sender.body["sender"]["platform_id"] = json!("ou_another");
                sender.raw_mut()["event"]["sender"]["sender_id"]["open_id"] = json!("ou_another");
                chat.body["conversation"]["platform_id"] = json!("oc_another");
                chat.raw_mut()["event"]["message"]["chat_id"] = json!("oc_another");
                private.raw_mut()["event"]["message"]["chat_type"] = json!("p2p");
            }
            BridgePlatform::Discord => {
                sender.body["sender"]["platform_id"] = json!("another-sender");
                sender.raw_mut()["d"]["author"]["id"] = json!("another-sender");
                chat.body["conversation"]["platform_id"] = json!("another-chat");
                chat.raw_mut()["d"]["channel_id"] = json!("another-chat");
                private.raw_mut()["d"]
                    .as_object_mut()
                    .unwrap()
                    .remove("guild_id");
                private.raw_mut()["d"]["channel"] = json!({"id":"discord-chat","type":1});
            }
            BridgePlatform::Slack | BridgePlatform::Whatsapp | BridgePlatform::X => {
                unreachable!("separate boundary fixtures")
            }
        }
        assert_ne!(initial, sender.key());
        assert_ne!(initial, chat.key());
        assert_ne!(initial, private.key());
        assert_eq!(
            private.write().unwrap().record().envelope.conversation_type,
            "private"
        );
    }
}

#[test]
fn signed_stable_threads_require_actual_supported_fields_and_ignore_generic_context() {
    for platform in platforms() {
        let mut fixture = Fixture::new(platform.clone());
        fixture.config_mut().partition = BridgePartitionPolicy::ThreadSender;
        match platform {
            BridgePlatform::Telegram | BridgePlatform::TelegramNew => {
                fixture.raw_mut()["message"]["message_thread_id"] = json!(42);
                fixture.raw_mut()["message"]["is_topic_message"] = json!(true);
            }
            BridgePlatform::Lark | BridgePlatform::Feishu => {
                fixture.raw_mut()["event"]["message"]["thread_id"] = json!("ot_stable-thread");
            }
            BridgePlatform::Discord => {
                fixture.raw_mut()["d"]["channel"] = json!({"id":"discord-chat","type":11});
            }
            BridgePlatform::Slack | BridgePlatform::Whatsapp | BridgePlatform::X => {
                unreachable!("separate boundary fixtures")
            }
        }
        let key = fixture.key();
        fixture.body["thread_id"] = json!("interaction:application:fixture-secret");
        assert_eq!(key, fixture.key());
        let encoded = serde_json::to_string(fixture.write().unwrap().record()).unwrap();
        assert!(!encoded.contains("fixture-secret"));
        assert!(!encoded.contains("interaction:"));
    }
    let mut discord = Fixture::new(BridgePlatform::Discord);
    discord.config_mut().partition = BridgePartitionPolicy::Thread;
    discord.raw_mut()["d"]["thread"] = json!({"id":"child-created-from-parent"});
    discord.body["thread_id"] = json!("interaction:application:fixture-secret");
    deny(&discord, PartitionRefusal::ThreadEvidenceUnavailable);
    let mut telegram = Fixture::new(BridgePlatform::Telegram);
    telegram.config_mut().partition = BridgePartitionPolicy::Thread;
    telegram.raw_mut()["message"]["message_thread_id"] = json!(42);
    deny(&telegram, PartitionRefusal::ThreadEvidenceUnavailable);
}

#[test]
fn signed_raw_identity_and_text_substitution_never_establish_human_admission() {
    for platform in platforms() {
        for field in ["chat", "sender", "text"] {
            let mut fixture = Fixture::new(platform.clone());
            match field {
                "chat" => fixture.body["conversation"]["platform_id"] = json!("other-chat"),
                "sender" => fixture.body["sender"]["platform_id"] = json!("other-sender"),
                _ => fixture.body["content"]["text"] = json!("replaced text"),
            }
            if field == "text" {
                deny(&fixture, PartitionRefusal::UnknownEvent);
            } else {
                assert!(matches!(fixture.write(), Err(CallbackError::Payload)));
            }
        }
    }
    let mut fixture = Fixture::new(BridgePlatform::Telegram);
    fixture.body["sender"]["platform_id"] = json!("012345");
    assert!(matches!(fixture.write(), Err(CallbackError::Payload)));
}

#[test]
fn telegram_new_delegates_only_wire_shapes_and_keeps_platform_and_partition_identity_distinct() {
    let legacy = Fixture::new(BridgePlatform::Telegram);
    let mut managed = Fixture::new(BridgePlatform::TelegramNew);
    assert_eq!(managed.body["platform"], "telegram-new");
    assert_eq!(
        managed.body["raw_platform_data"],
        legacy.body["raw_platform_data"]
    );
    assert_ne!(legacy.key(), managed.key());
    let record = serde_json::to_value(managed.write().unwrap().record()).unwrap();
    assert_eq!(record["capabilities"]["platform"], "telegram-new");
    assert_eq!(record["envelope"]["platform"], "telegram-new");
    managed.config_mut().connector.platform = BridgePlatform::Telegram;
    assert!(managed.write().is_err());
    let mut alias = Fixture::new(BridgePlatform::TelegramNew);
    alias.body["platform"] = json!("telegram");
    assert!(alias.write().is_err());
}

#[test]
fn telegram_new_unverified_threads_edits_actions_and_loops_remain_inert() {
    let original = Fixture::new(BridgePlatform::TelegramNew);
    let mut fixture = original.clone();
    fixture.config_mut().partition = BridgePartitionPolicy::Thread;
    fixture.raw_mut()["message"]["message_thread_id"] = json!(42);
    deny(&fixture, PartitionRefusal::ThreadEvidenceUnavailable);
    fixture.raw_mut()["message"]["is_topic_message"] = json!(true);
    fixture.raw_mut()["message"]["chat"]["type"] = json!("private");
    deny(&fixture, PartitionRefusal::ThreadEvidenceUnavailable);
    let mut fixture = original.clone();
    let message = fixture
        .raw_mut()
        .as_object_mut()
        .unwrap()
        .remove("message")
        .unwrap();
    fixture.raw_mut()["edited_message"] = message;
    deny(&fixture, PartitionRefusal::MessageEdit);
    let mut fixture = original.clone();
    fixture.body["raw_platform_data"] = json!({"callback_query":{"data":"untrusted-command"}});
    deny(&fixture, PartitionRefusal::InteractiveActionUnsupported);
    let mut fixture = original;
    fixture.raw_mut()["message"]["via_bot"] = json!({"id":123});
    deny(&fixture, PartitionRefusal::CallbackLoop);
}
