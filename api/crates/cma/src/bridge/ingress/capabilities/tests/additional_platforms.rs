//! Pinned boundary fixtures. Missing human/loop observations never admit work.
use super::*;

fn additional() -> [BridgePlatform; 3] {
    [
        BridgePlatform::Slack,
        BridgePlatform::Whatsapp,
        BridgePlatform::X,
    ]
}

fn facts(fixture: &Fixture) -> PlatformCapabilityObservation {
    let body = fixture.bytes();
    let signed = fixture.signed().unwrap();
    NyxidCapabilities
        .observe(&signed.authenticated_payload(&body).unwrap())
        .unwrap()
}

#[test]
fn signed_text_preserves_provider_identity_without_inventing_human_or_loop_evidence() {
    for (platform, message, conversation) in [
        (
            BridgePlatform::Slack,
            "1700000000.000001",
            ChannelConversationKind::Channel,
        ),
        (
            BridgePlatform::Whatsapp,
            "wamid.fixture",
            ChannelConversationKind::Private,
        ),
        (BridgePlatform::X, "100", ChannelConversationKind::Private),
    ] {
        let fixture = Fixture::new(platform);
        let observed = facts(&fixture);
        assert_eq!(observed.provider_message_id.as_deref(), Some(message));
        assert_eq!(observed.conversation, conversation);
        assert_eq!(observed.event_kind, ChannelEventKind::UserMessage);
        assert_eq!(observed.sender_kind, ChannelSenderKind::Unknown);
        assert_eq!(observed.loop_evidence, LoopEvidence::Unknown);
        assert_eq!(observed.mentioned_bot, VerifiedFlag::Unknown);
        assert_eq!(observed.replied_to_bot, VerifiedFlag::Unknown);
        deny(&fixture, PartitionRefusal::UnknownSender);
        let bytes = fixture.bytes();
        let signed = fixture.signed().unwrap();
        let payload = signed.authenticated_payload(&bytes).unwrap();
        assert!(crate::bridge::ingress::redelivery::observe(&payload)
            .unwrap()
            .is_none());
        assert!(
            crate::bridge::ingress::reply_addressing::ReplyLookup::authenticated(&payload)
                .unwrap()
                .is_none()
        );
        let write = fixture.write().unwrap();
        assert!(write.control_refusal().is_none());
        let stored: StoredInbox =
            serde_json::from_slice(&serde_json::to_vec(write.record()).unwrap()).unwrap();
        assert_eq!(stored.admission, write.record().admission);
    }
}

#[test]
fn raw_identity_must_match_the_signed_normalized_chat_and_sender() {
    for platform in additional() {
        for field in ["chat", "sender"] {
            let mut fixture = Fixture::new(platform.clone());
            if field == "chat" {
                fixture.body["conversation"]["platform_id"] = json!("different-chat");
            } else {
                fixture.body["sender"]["platform_id"] = json!("different-sender");
            }
            assert!(matches!(fixture.write(), Err(CallbackError::Payload)));
        }
        let mut fixture = Fixture::new(platform);
        fixture.body["content"]["text"] = json!("CMA-CONTROL/1 cancel ccr_unproven");
        assert_eq!(facts(&fixture).event_kind, ChannelEventKind::Unknown);
        deny(&fixture, PartitionRefusal::UnknownSender);
    }
}

#[test]
fn conversation_and_thread_facts_come_only_from_actual_provider_fields() {
    for (chat, expected) in [
        ("D123", ChannelConversationKind::Private),
        ("G123", ChannelConversationKind::Group),
        ("C123", ChannelConversationKind::Channel),
        ("future", ChannelConversationKind::Unknown),
    ] {
        let mut fixture = Fixture::new(BridgePlatform::Slack);
        fixture.body["conversation"]["platform_id"] = json!(chat);
        fixture.raw_mut()["event"]["channel"] = json!(chat);
        assert_eq!(facts(&fixture).conversation, expected);
        fixture.body["thread_id"] = json!("generic-unverified-thread");
        assert!(matches!(facts(&fixture).thread, ThreadEvidence::Missing));
        fixture.raw_mut()["event"]["thread_ts"] = json!("1700000000.000000");
        let ThreadEvidence::Verified(thread) = facts(&fixture).thread else {
            panic!("expected stable thread")
        };
        assert_eq!(thread.kind(), StableThreadKind::SlackThread);
        assert_eq!(thread.id(), "1700000000.000000");
        deny(&fixture, PartitionRefusal::UnknownSender);
    }
    for platform in [BridgePlatform::Whatsapp, BridgePlatform::X] {
        let mut fixture = Fixture::new(platform.clone());
        fixture.body["thread_id"] = json!("generic-unverified-thread");
        assert!(matches!(
            facts(&fixture).thread,
            ThreadEvidence::Unsupported
        ));
        if platform == BridgePlatform::X {
            fixture.body["conversation"]["platform_id"] = json!("999");
            fixture.raw_mut()["dm_conversation_id"] = json!("999");
            assert_eq!(facts(&fixture).conversation, ChannelConversationKind::Group);
        }
    }
}

#[test]
fn reply_parent_fields_preserve_roots_and_never_treat_shared_posts_as_dm_replies() {
    for (platform, parent) in [
        (BridgePlatform::Slack, "1700000000.000000"),
        (BridgePlatform::Whatsapp, "wamid.parent"),
        (BridgePlatform::X, "99"),
    ] {
        let mut fixture = Fixture::new(platform.clone());
        assert_eq!(
            provider_parent(&platform, &fixture.body["raw_platform_data"]).unwrap(),
            None
        );
        match platform {
            BridgePlatform::Slack => fixture.raw_mut()["event"]["thread_ts"] = json!(parent),
            BridgePlatform::Whatsapp => fixture.raw_mut()["context"] = json!({"id":parent}),
            BridgePlatform::X => {
                fixture.raw_mut()["referenced_tweets"] =
                    json!([{"type":"replied_to","id":"shared-post"}]);
                assert_eq!(
                    provider_parent(&platform, &fixture.body["raw_platform_data"]).unwrap(),
                    None
                );
                fixture.raw_mut()["referenced_events"] = json!([{"type":"replied_to","id":parent}]);
            }
            _ => unreachable!(),
        }
        assert_eq!(
            provider_parent(&platform, &fixture.body["raw_platform_data"])
                .unwrap()
                .as_deref(),
            Some(parent)
        );
        fixture.body["reply_to_platform_message_id"] = json!(parent);
        deny(&fixture, PartitionRefusal::UnknownSender);
        if platform == BridgePlatform::Slack {
            let root = fixture.body["raw_platform_data"]["event"]["ts"].clone();
            fixture.raw_mut()["event"]["thread_ts"] = root;
            assert_eq!(
                provider_parent(&platform, &fixture.body["raw_platform_data"]).unwrap(),
                None
            );
        }
    }
}

#[test]
fn nontext_events_cannot_be_promoted_by_text_or_reserved_control_fields() {
    for text in ["hello", "CMA-CONTROL/1 approve ccr_unproven"] {
        for platform in additional() {
            let mut fixture = Fixture::new(platform.clone());
            fixture.body["content"]["text"] = json!(text);
            match platform {
                BridgePlatform::Slack => {
                    fixture.raw_mut()["event"]["text"] = json!(text);
                    fixture.raw_mut()["event"]["files"] = json!([{"id":"file-1"}]);
                }
                BridgePlatform::Whatsapp => {
                    fixture.raw_mut()["type"] = json!("image");
                    fixture.raw_mut()["image"] = json!({"id":"123","caption":text});
                }
                BridgePlatform::X => {
                    fixture.raw_mut()["text"] = json!(text);
                    fixture.raw_mut()["attachments"] = json!({"media_keys":["3_media"]});
                }
                _ => unreachable!(),
            }
            assert_eq!(
                facts(&fixture).event_kind,
                ChannelEventKind::AttachmentMessage
            );
            deny(&fixture, PartitionRefusal::UnknownSender);
            assert!(fixture.write().unwrap().control_refusal().is_none());
        }
    }
    for kind in ["button", "interactive", "location", "contacts", "order"] {
        let mut fixture = Fixture::new(BridgePlatform::Whatsapp);
        fixture.raw_mut()["type"] = json!(kind);
        assert_eq!(
            facts(&fixture).event_kind,
            if matches!(kind, "button" | "interactive") {
                ChannelEventKind::InteractiveAction
            } else {
                ChannelEventKind::UnsupportedContent
            }
        );
        assert_ne!(
            fixture.write().unwrap().record().admission,
            InboxAdmission::Accepted
        );
    }
}

#[test]
fn slack_bots_edits_and_missing_discriminators_keep_named_refusals() {
    let mut bot = Fixture::new(BridgePlatform::Slack);
    bot.raw_mut()["event"]["bot_id"] = json!("B123");
    deny(&bot, PartitionRefusal::BotSender);
    let mut edit = Fixture::new(BridgePlatform::Slack);
    edit.raw_mut()["event"]["subtype"] = json!("message_changed");
    deny(&edit, PartitionRefusal::MessageEdit);
    for platform in additional() {
        let mut fixture = Fixture::new(platform);
        fixture
            .body
            .as_object_mut()
            .unwrap()
            .remove("raw_platform_data");
        assert_eq!(facts(&fixture).event_kind, ChannelEventKind::Unknown);
        deny(&fixture, PartitionRefusal::UnknownSender);
    }
}
