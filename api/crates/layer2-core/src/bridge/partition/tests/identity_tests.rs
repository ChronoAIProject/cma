use super::*;

#[test]
fn default_partitions_keep_senders_chats_conversations_and_platform_scopes_separate() {
    assert_eq!(
        BridgePartitionPolicy::default(),
        BridgePartitionPolicy::ConversationSender
    );
    let mut platform_keys = std::collections::BTreeSet::new();
    for platform in platforms() {
        let base = Fixture::new(platform, BridgePartitionPolicy::default());
        let original = base.key();
        platform_keys.insert(original.hash().to_owned());
        let mut second_sender = base.clone();
        second_sender.inbox.envelope.sender_id = "sender-two".into();
        assert_ne!(original, second_sender.key());
        let mut second_chat = base.clone();
        second_chat.inbox.envelope.chat_id = "chat-two".into();
        assert_ne!(original, second_chat.key());
        assert_eq!(
            base.inbox.envelope.route_id,
            second_chat.inbox.envelope.route_id
        );
        let mut private = base.clone();
        private.inbox.envelope.conversation_type = "private".into();
        assert_ne!(original, private.key());
        for dimension in ["issuer", "bridge", "bot"] {
            let mut changed = base.clone();
            match dimension {
                "issuer" => {
                    changed.config_mut().connector.issuer = "https://other.example".into();
                    changed.inbox.attestation.issuer = "https://other.example".into();
                }
                "bridge" => {
                    changed.bridge.id = BridgeId("brg_other".into());
                    changed.inbox.pin.bridge_id = changed.bridge.id.clone();
                }
                _ => {
                    changed.config_mut().connector.bot_id = "bot-two".into();
                    changed.inbox.bot_id = "bot-two".into();
                }
            }
            assert_ne!(original, changed.key(), "{dimension}");
        }
    }
    assert_eq!(platform_keys.len(), 5);
}

#[test]
fn display_route_message_and_configuration_changes_do_not_reset_context() {
    let mut fixture = Fixture::new(BridgePlatform::Telegram, BridgePartitionPolicy::default());
    let original = fixture.key();
    fixture.inbox.envelope.sender_display_name = Some("Renamed sender".into());
    fixture.inbox.envelope.route_id = "another-route".into();
    fixture.inbox.envelope.text = Some("A later message".into());
    fixture.inbox.envelope.protected_thread_context_present = true;
    fixture.inbox.envelope.reply_to_platform_message_id = Some("reply-to-another".into());
    fixture.inbox.message_id = "message-two".into();
    fixture.inbox.id = "3".repeat(64);
    fixture.inbox.attestation.body_sha256 = "4".repeat(64);
    let mut generation = fixture.bridge.generations[0].clone();
    generation.config_revision = 2;
    generation.config.display_name = "Renamed bridge".into();
    generation.previous_work = Some(AcceptedWorkPolicy::DrainOriginal);
    fixture.bridge.generations.push(generation);
    fixture.bridge.config_revision = 2;
    fixture.bridge.revision = 2;
    fixture.inbox.pin.config_revision = 2;
    assert_eq!(original, fixture.key());
    assert_eq!(
        fixture
            .source(fixture.facts())
            .source_pin()
            .work_pin()
            .config_revision,
        2
    );
}

#[test]
fn opaque_id_boundaries_preserve_leading_zero_unicode_and_separator_identity() {
    let base = Fixture::new(BridgePlatform::Lark, BridgePartitionPolicy::default());
    for (left, right) in [("001", "1"), ("é", "e\u{301}"), ("sender:a", "sender/a")] {
        let mut one = base.clone();
        let mut two = base.clone();
        one.inbox.envelope.sender_id = left.into();
        two.inbox.envelope.sender_id = right.into();
        assert_ne!(one.key(), two.key());
    }
    let mut one = base.clone();
    let mut two = base;
    one.inbox.envelope.chat_id = "chat:a".into();
    one.inbox.envelope.sender_id = "b".into();
    two.inbox.envelope.chat_id = "chat".into();
    two.inbox.envelope.sender_id = "a:b".into();
    assert_ne!(one.key(), two.key());
}

#[test]
fn explicit_context_policies_share_only_their_documented_dimensions_and_keep_group_replies() {
    for policy in [
        BridgePartitionPolicy::Conversation,
        BridgePartitionPolicy::SenderAcrossChats,
        BridgePartitionPolicy::FixedSharedAgent,
    ] {
        let base = Fixture::new(BridgePlatform::Telegram, policy);
        let mut other_sender = base.clone();
        other_sender.inbox.envelope.sender_id = "sender-two".into();
        let mut other_chat = base.clone();
        other_chat.inbox.envelope.chat_id = "chat-two".into();
        match policy {
            BridgePartitionPolicy::Conversation => {
                assert_eq!(base.key(), other_sender.key());
                assert_ne!(base.key(), other_chat.key());
            }
            BridgePartitionPolicy::SenderAcrossChats => {
                assert_ne!(base.key(), other_sender.key());
                assert_eq!(base.key(), other_chat.key());
            }
            _ => {
                assert_eq!(base.key(), other_sender.key());
                assert_eq!(base.key(), other_chat.key());
            }
        }
        assert!(
            matches!(base.decision(base.facts()), PartitionAdmissionDecision::Admitted { reply_visibility: ReplyVisibility::OriginalGroupOrChannel, context, .. } if context == policy.into())
        );
    }
}

#[test]
fn stable_thread_keys_require_the_right_platform_capability_and_do_not_use_generic_context() {
    for platform in platforms() {
        for policy in [
            BridgePartitionPolicy::Thread,
            BridgePartitionPolicy::ThreadSender,
        ] {
            let base = Fixture::new(platform.clone(), policy);
            let mut facts = base.facts();
            facts.thread = stable_thread(&platform, "opaque-thread:one");
            let original = base.key_with(facts.clone());
            facts.thread = stable_thread(&platform, "opaque-thread:two");
            assert_ne!(original, base.key_with(facts));
            let mut sender = base.clone();
            sender.inbox.envelope.sender_id = "sender-two".into();
            let mut facts = sender.facts();
            facts.thread = stable_thread(&platform, "opaque-thread:one");
            assert_eq!(
                original == sender.key_with(facts),
                policy == BridgePartitionPolicy::Thread
            );
            let mut wrong = base.facts();
            wrong.thread = stable_thread(
                if matches!(
                    platform,
                    BridgePlatform::Telegram | BridgePlatform::TelegramNew
                ) {
                    &BridgePlatform::Discord
                } else {
                    &BridgePlatform::Telegram
                },
                "opaque-thread:one",
            );
            assert!(matches!(
                NormalizedPartitionSource::from_observation(&base.inbox, &base.bridge, wrong),
                Err(PartitionError::InvalidSource("stable_thread_platform"))
            ));
        }
    }
    let raw = "interaction:application:secret-value";
    let result = StableThreadReference::from_verified_field(
        StableThreadKind::DiscordChannelThread,
        raw.into(),
    );
    assert!(matches!(
        &result,
        Err(PartitionError::InvalidSource("stable_thread"))
    ));
    if let Err(error) = result {
        assert!(!error.to_string().contains("secret-value"));
    }
}

#[test]
fn substituted_message_observation_or_configuration_cannot_be_admitted() {
    let base = Fixture::new(BridgePlatform::Telegram, BridgePartitionPolicy::default());
    for dimension in [
        "inbox",
        "body",
        "config",
        "chat",
        "sender",
        "platform",
        "conversation",
    ] {
        let mut facts = base.facts();
        match dimension {
            "inbox" | "body" | "config" => {
                let mut different = base.inbox.clone();
                match dimension {
                    "inbox" => different.id = "3".repeat(64),
                    "body" => different.attestation.body_sha256 = "4".repeat(64),
                    _ => different.pin.config_revision = 2,
                }
                facts.source = PartitionSourcePin::for_inbox(&different).unwrap();
            }
            "chat" => facts.chat_id = "other-chat".into(),
            "sender" => facts.sender_id = "other-sender".into(),
            "platform" => facts.platform = BridgePlatform::Discord,
            _ => facts.conversation = ChannelConversationKind::Private,
        }
        assert!(
            matches!(
                NormalizedPartitionSource::from_observation(&base.inbox, &base.bridge, facts),
                Err(PartitionError::SourceMismatch)
            ),
            "{dimension}"
        );
    }
    let source = base.source(base.facts());
    let mut newer = base.config().clone();
    newer.display_name = "New configuration".into();
    assert_eq!(source.admit(&newer), Err(PartitionError::SourceMismatch));
}

#[test]
fn serialized_partition_identity_is_bounded_validated_and_contains_no_source_details() {
    let fixture = Fixture::new(BridgePlatform::Telegram, BridgePartitionPolicy::default());
    let key = fixture.key();
    let encoded = serde_json::to_string(&key).unwrap();
    assert_eq!(key.version(), 1);
    assert_eq!(
        key.hash(),
        "d63de9cbb18c83547422fe172ae7b590b91c8c8aafaaf884d633966d638d5ac3"
    );
    assert_eq!(encoded.len(), 87);
    for private in [
        "sender-one",
        "chat-one",
        "Display name",
        "Keep this out",
        "bot-one",
        "brg_fixture",
    ] {
        assert!(!encoded.contains(private));
        assert!(!format!("{key:?}").contains(private));
    }
    assert_eq!(
        key,
        serde_json::from_str::<NormalizedPartitionKey>(&encoded).unwrap()
    );
    let pin = PartitionSourcePin::for_inbox(&fixture.inbox).unwrap();
    assert_eq!(
        pin,
        serde_json::from_value::<PartitionSourcePin>(serde_json::to_value(&pin).unwrap()).unwrap()
    );
    for bad in [
        serde_json::json!({"version":2,"hash":key.hash()}),
        serde_json::json!({"version":1,"hash":"secret"}),
        serde_json::json!({"version":1,"hash":key.hash(),"chat_id":"secret"}),
    ] {
        assert!(serde_json::from_value::<NormalizedPartitionKey>(bad).is_err());
    }
    let mut invalid = serde_json::to_value(pin).unwrap();
    invalid["work_pin"]["config_revision"] = 0.into();
    assert!(serde_json::from_value::<PartitionSourcePin>(invalid).is_err());
}
