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
            BridgePlatform::Telegram => {
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
        deny(&fixture, PartitionRefusal::UnknownEvent);
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
