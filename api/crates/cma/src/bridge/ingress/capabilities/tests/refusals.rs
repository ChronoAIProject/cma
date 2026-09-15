use super::*;

#[test]
fn signed_sender_chat_and_group_gates_remain_independent_and_default_closed() {
    for platform in platforms() {
        let mut fixture = Fixture::new(platform.clone());
        fixture.body["thread_id"] = json!("interaction:application:discard-on-denial");
        fixture.config_mut().admission = BridgeAdmissionPolicy::Scoped {
            policy: ChannelAdmissionPolicy::default(),
        };
        deny(&fixture, PartitionRefusal::SenderDenied);
        let sender = fixture.body["sender"]["platform_id"]
            .as_str()
            .unwrap()
            .to_string();
        let chat = fixture.body["conversation"]["platform_id"]
            .as_str()
            .unwrap()
            .to_string();
        fixture.config_mut().admission = BridgeAdmissionPolicy::Scoped {
            policy: ChannelAdmissionPolicy {
                senders: IdentityAdmission::Allowlist { ids: vec![sender] },
                chats: IdentityAdmission::Allowlist {
                    ids: vec!["another-chat".into()],
                },
                groups: GroupAdmission::All,
            },
        };
        deny(&fixture, PartitionRefusal::ChatDenied);
        if let BridgeAdmissionPolicy::Scoped { policy } = &mut fixture.config_mut().admission {
            policy.chats = IdentityAdmission::Allowlist { ids: vec![chat] };
        }
        assert_eq!(
            fixture.write().unwrap().record().admission,
            InboxAdmission::Accepted
        );
        for group in [
            GroupAdmission::Mention,
            GroupAdmission::ReplyToBot,
            GroupAdmission::MentionOrReplyToBot,
        ] {
            if let BridgeAdmissionPolicy::Scoped { policy } = &mut fixture.config_mut().admission {
                policy.groups = group;
            }
            deny(&fixture, PartitionRefusal::AddressingEvidenceUnavailable);
        }
    }
}

#[test]
fn bot_unknown_sender_and_callback_loops_cannot_override_explicit_open_admission() {
    for platform in platforms() {
        let mut bot = Fixture::new(platform.clone());
        let mut unknown = bot.clone();
        match platform {
            BridgePlatform::Telegram | BridgePlatform::TelegramNew => {
                bot.raw_mut()["message"]["from"]["is_bot"] = json!(true);
                unknown.raw_mut()["message"]["from"]
                    .as_object_mut()
                    .unwrap()
                    .remove("is_bot");
            }
            BridgePlatform::Lark | BridgePlatform::Feishu => {
                bot.raw_mut()["event"]["sender"]["sender_type"] = json!("app");
                unknown.raw_mut()["event"]["sender"]
                    .as_object_mut()
                    .unwrap()
                    .remove("sender_type");
            }
            BridgePlatform::Discord => {
                bot.raw_mut()["d"]["author"]["bot"] = json!(true);
                unknown.raw_mut()["d"]["author"]
                    .as_object_mut()
                    .unwrap()
                    .remove("bot");
            }
        }
        deny(&bot, PartitionRefusal::BotSender);
        deny(&unknown, PartitionRefusal::UnknownSender);
        let mut generic = Fixture::new(platform);
        generic
            .body
            .as_object_mut()
            .unwrap()
            .remove("raw_platform_data");
        deny(&generic, PartitionRefusal::UnknownSender);
    }
    let mut telegram = Fixture::new(BridgePlatform::Telegram);
    telegram.raw_mut()["message"]["via_bot"] = json!({"id":987,"is_bot":true});
    deny(&telegram, PartitionRefusal::CallbackLoop);
    let mut discord = Fixture::new(BridgePlatform::Discord);
    discord.raw_mut()["d"]["webhook_id"] = json!("webhook-echo");
    deny(&discord, PartitionRefusal::CallbackLoop);
}

#[test]
fn edits_interactive_actions_unknown_events_and_nontext_do_not_start_agent_turns() {
    let mut telegram = Fixture::new(BridgePlatform::Telegram);
    let message = telegram
        .raw_mut()
        .as_object_mut()
        .unwrap()
        .remove("message")
        .unwrap();
    telegram.raw_mut()["edited_message"] = message;
    deny(&telegram, PartitionRefusal::MessageEdit);
    for platform in [BridgePlatform::Lark, BridgePlatform::Feishu] {
        let mut fixture = Fixture::new(platform);
        fixture.raw_mut()["header"]["event_type"] = json!("card.action.trigger");
        deny(&fixture, PartitionRefusal::InteractiveActionUnsupported);
    }
    let mut discord = Fixture::new(BridgePlatform::Discord);
    discord.raw_mut()["t"] = json!("MESSAGE_UPDATE");
    deny(&discord, PartitionRefusal::MessageEdit);
    discord.body["raw_platform_data"] = json!({"type":2,"id":"interaction-id","application_id":"app",
        "token":"fixture-interaction-secret","member":{"user":{"id":"discord-sender","bot":false}},
        "channel_id":"discord-chat","channel":{"type":11},"data":{"name":"dangerous-command"}});
    discord.body["thread_id"] = json!("interaction:app:fixture-interaction-secret");
    deny(&discord, PartitionRefusal::InteractiveActionUnsupported);
    let record = serde_json::to_string(discord.write().unwrap().record()).unwrap();
    assert!(!record.contains("fixture-interaction-secret"));
    assert!(!record.contains("dangerous-command"));
    for platform in platforms() {
        let mut fixture = Fixture::new(platform);
        fixture.body["content"]["type"] = json!("image");
        deny(&fixture, PartitionRefusal::ContentUnsupported);
    }
}

#[test]
fn raw_group_addressing_hints_do_not_infer_selected_bot_identity() {
    let mut telegram = Fixture::new(BridgePlatform::Telegram);
    telegram.raw_mut()["message"]["entities"] = json!([{"type":"mention","offset":0,"length":5}]);
    telegram.raw_mut()["message"]["reply_to_message"] =
        json!({"message_id":40,"from":{"id":999,"is_bot":true}});
    let mut lark = Fixture::new(BridgePlatform::Lark);
    lark.raw_mut()["event"]["message"]["mentions"] =
        json!([{"id":{"open_id":"ou_bot"},"name":"Selected bot"}]);
    lark.raw_mut()["event"]["message"]["parent_id"] = json!("om_parent");
    for mut fixture in [telegram, lark] {
        if let BridgeAdmissionPolicy::Scoped { policy } = &mut fixture.config_mut().admission {
            policy.groups = GroupAdmission::MentionOrReplyToBot;
        }
        deny(&fixture, PartitionRefusal::AddressingEvidenceUnavailable);
    }
}

#[test]
fn attachments_are_precisely_refused_without_text_fallback_or_reply_custody() {
    for platform in platforms() {
        let mut fixture = Fixture::new(platform.clone());
        fixture.body["content"]["attachments"] = json!([{"content_type":"image","url":"","image_key":"private-reference","size_bytes":32}]);
        fixture.body["thread_id"] = json!("interaction:app:private-context");
        deny(&fixture, PartitionRefusal::AttachmentDownloadRequired);
        let wire = serde_json::to_value(fixture.write().unwrap().record()).unwrap();
        assert_eq!(wire["capabilities"]["event_kind"], "attachment_message");
        assert!(!wire.to_string().contains("private-context"));
        let retained: StoredInbox = serde_json::from_value(wire).unwrap();
        assert!(NormalizedPartitionSource::from_stored(&retained, &fixture.bridge).is_ok());
        assert_eq!(
            retained.admission,
            InboxAdmission::PartitionDenied {
                reason: PartitionRefusal::AttachmentDownloadRequired
            }
        );
        // Missing normalized media must not turn an original attachment into text input.
        fixture.body["content"]
            .as_object_mut()
            .unwrap()
            .remove("attachments");
        match platform {
            BridgePlatform::Telegram | BridgePlatform::TelegramNew => {
                fixture.raw_mut()["message"]["photo"] = json!([{"file_id":"private-id"}])
            }
            BridgePlatform::Discord => {
                fixture.raw_mut()["d"]["attachments"] = json!([{"id":"private-id"}])
            }
            _ => fixture.raw_mut()["event"]["message"]["message_type"] = json!("image"),
        }
        deny(&fixture, PartitionRefusal::AttachmentDownloadRequired);
    }
}

#[test]
fn attachment_refusals_preserve_sender_edit_action_loop_and_signature_precedence() {
    let mut fixture = Fixture::new(BridgePlatform::Telegram);
    fixture.body["content"]["attachments"] =
        json!([{"content_type":"file","url":"","file_key":"opaque"}]);
    fixture.raw_mut()["message"]["via_bot"] = json!({"id":77});
    deny(&fixture, PartitionRefusal::CallbackLoop);
    fixture.raw_mut()["message"]["from"]["is_bot"] = json!(true);
    deny(&fixture, PartitionRefusal::BotSender);
    let message = fixture
        .raw_mut()
        .as_object_mut()
        .unwrap()
        .remove("message")
        .unwrap();
    fixture.raw_mut()["edited_message"] = message;
    deny(&fixture, PartitionRefusal::MessageEdit);
    fixture.body["raw_platform_data"] = json!({"callback_query":{"data":"do-something"}});
    deny(&fixture, PartitionRefusal::InteractiveActionUnsupported);
    let signed = fixture.signed().unwrap();
    fixture.body["content"]["attachments"][0]["file_key"] = json!("unsigned");
    assert!(matches!(
        signed.classify_capabilities(&fixture.bytes(), &NyxidCapabilities),
        Err(CallbackError::Authentication)
    ));
}
