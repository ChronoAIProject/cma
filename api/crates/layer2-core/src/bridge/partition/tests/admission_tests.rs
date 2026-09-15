use super::*;

#[test]
fn unknown_sender_event_and_loop_evidence_are_not_inferred_from_generic_callbacks() {
    for platform in platforms() {
        let fixture = Fixture::new(platform, BridgePartitionPolicy::default());
        let unknown = PlatformCapabilityObservation::unknown(&fixture.inbox).unwrap();
        assert_eq!(
            fixture.decision(unknown),
            refused(PartitionRefusal::UnknownSender)
        );
        for (sender, reason) in [
            (ChannelSenderKind::Bot, PartitionRefusal::BotSender),
            (ChannelSenderKind::System, PartitionRefusal::SystemSender),
        ] {
            let mut facts = fixture.facts();
            facts.sender_kind = sender;
            assert_eq!(fixture.decision(facts), refused(reason));
        }
        for (event, reason) in [
            (ChannelEventKind::Unknown, PartitionRefusal::UnknownEvent),
            (ChannelEventKind::SystemEvent, PartitionRefusal::SystemEvent),
            (
                ChannelEventKind::CallbackEcho,
                PartitionRefusal::CallbackLoop,
            ),
        ] {
            let mut facts = fixture.facts();
            facts.event_kind = event;
            assert_eq!(fixture.decision(facts), refused(reason));
        }
        for (evidence, reason) in [
            (LoopEvidence::Loop, PartitionRefusal::CallbackLoop),
            (LoopEvidence::Unknown, PartitionRefusal::UnknownLoopEvidence),
        ] {
            let mut facts = fixture.facts();
            facts.loop_evidence = evidence;
            assert_eq!(fixture.decision(facts), refused(reason));
        }
        let mut unsupported = fixture;
        unsupported.inbox.envelope.conversation_type = "unsupported-device".into();
        assert_eq!(
            unsupported.decision(unsupported.facts()),
            refused(PartitionRefusal::UnsupportedConversation)
        );
    }
}

#[test]
fn scoped_sender_and_chat_gates_intersect_and_safe_defaults_refuse() {
    let mut fixture = Fixture::new(BridgePlatform::Lark, BridgePartitionPolicy::default());
    fixture.config_mut().admission = BridgeAdmissionPolicy::Scoped {
        policy: ChannelAdmissionPolicy::default(),
    };
    assert_eq!(
        fixture.decision(fixture.facts()),
        refused(PartitionRefusal::SenderDenied)
    );
    fixture.inbox.envelope.sender_id = "opaque/发送者:001".into();
    let policy = ChannelAdmissionPolicy {
        senders: IdentityAdmission::Allowlist {
            ids: vec!["opaque/发送者:001".into()],
        },
        chats: IdentityAdmission::Allowlist {
            ids: vec!["chat-one".into()],
        },
        groups: GroupAdmission::All,
    };
    fixture.config_mut().admission = BridgeAdmissionPolicy::Scoped { policy };
    assert!(matches!(
        fixture.decision(fixture.facts()),
        PartitionAdmissionDecision::Admitted { .. }
    ));
    fixture.inbox.envelope.chat_id = "chat-two".into();
    assert_eq!(
        fixture.decision(fixture.facts()),
        refused(PartitionRefusal::ChatDenied)
    );
    fixture.inbox.envelope.sender_id = "opaque/发送者:1".into();
    assert_eq!(
        fixture.decision(fixture.facts()),
        refused(PartitionRefusal::SenderDenied)
    );
}

#[test]
fn group_mention_and_reply_rules_use_verified_selected_bot_facts_consistently() {
    use GroupAdmission as G;
    use VerifiedFlag as F;
    let cases = [
        (G::Deny, F::Yes, F::Yes, Some(PartitionRefusal::GroupDenied)),
        (G::All, F::Unknown, F::Unknown, None),
        (G::Mention, F::Yes, F::Unknown, None),
        (
            G::Mention,
            F::No,
            F::Yes,
            Some(PartitionRefusal::GroupNotAddressed),
        ),
        (
            G::Mention,
            F::Unknown,
            F::Yes,
            Some(PartitionRefusal::AddressingEvidenceUnavailable),
        ),
        (G::ReplyToBot, F::Unknown, F::Yes, None),
        (
            G::ReplyToBot,
            F::Yes,
            F::No,
            Some(PartitionRefusal::GroupNotAddressed),
        ),
        (
            G::ReplyToBot,
            F::Yes,
            F::Unknown,
            Some(PartitionRefusal::AddressingEvidenceUnavailable),
        ),
        (G::MentionOrReplyToBot, F::Yes, F::Unknown, None),
        (G::MentionOrReplyToBot, F::Unknown, F::Yes, None),
        (
            G::MentionOrReplyToBot,
            F::No,
            F::No,
            Some(PartitionRefusal::GroupNotAddressed),
        ),
        (
            G::MentionOrReplyToBot,
            F::No,
            F::Unknown,
            Some(PartitionRefusal::AddressingEvidenceUnavailable),
        ),
    ];
    for platform in platforms() {
        for (group, mentioned, replied, expected) in cases {
            let mut fixture = Fixture::new(platform.clone(), BridgePartitionPolicy::default());
            fixture.config_mut().admission = BridgeAdmissionPolicy::Scoped {
                policy: ChannelAdmissionPolicy {
                    senders: IdentityAdmission::Open,
                    chats: IdentityAdmission::Open,
                    groups: group,
                },
            };
            let mut facts = fixture.facts();
            facts.mentioned_bot = mentioned;
            facts.replied_to_bot = replied;
            let decision = fixture.decision(facts);
            if let Some(reason) = expected {
                assert_eq!(decision, refused(reason), "{platform:?} {group:?}");
            } else {
                assert!(matches!(
                    decision,
                    PartitionAdmissionDecision::Admitted { .. }
                ));
            }
            fixture.inbox.envelope.conversation_type = "private".into();
            assert!(matches!(
                fixture.decision(fixture.facts()),
                PartitionAdmissionDecision::Admitted {
                    reply_visibility: ReplyVisibility::OriginalPrivateChat,
                    ..
                }
            ));
        }
    }
}

#[test]
fn thread_policies_fail_visibly_without_a_supported_stable_reference() {
    for platform in platforms() {
        for policy in [
            BridgePartitionPolicy::Thread,
            BridgePartitionPolicy::ThreadSender,
        ] {
            let mut fixture = Fixture::new(platform.clone(), policy);
            fixture.inbox.envelope.protected_thread_context_present = true;
            for (thread, reason) in [
                (
                    ThreadEvidence::Unknown,
                    PartitionRefusal::ThreadEvidenceUnavailable,
                ),
                (
                    ThreadEvidence::Unsupported,
                    PartitionRefusal::ThreadUnsupported,
                ),
                (ThreadEvidence::Missing, PartitionRefusal::ThreadMissing),
            ] {
                let mut facts = fixture.facts();
                facts.thread = thread;
                assert_eq!(fixture.decision(facts), refused(reason));
            }
        }
    }
}

#[test]
fn legacy_policy_reads_remain_valid_but_new_fixed_targets_require_explicit_shared_context() {
    for policy in [
        BridgePartitionPolicy::Conversation,
        BridgePartitionPolicy::ConversationSender,
        BridgePartitionPolicy::Thread,
        BridgePartitionPolicy::ThreadSender,
        BridgePartitionPolicy::SenderAcrossChats,
    ] {
        let mut fixture = Fixture::new(
            BridgePlatform::Telegram,
            BridgePartitionPolicy::FixedSharedAgent,
        );
        fixture.config_mut().partition = policy;
        assert!(fixture.bridge.validate().is_ok());
        assert!(fixture.config().validate_partition_admission().is_err());
        assert!(matches!(
            fixture.source(fixture.facts()).admit(fixture.config()),
            Err(PartitionError::InvalidPolicy(_))
        ));
    }
    let fixed = Fixture::new(
        BridgePlatform::Telegram,
        BridgePartitionPolicy::FixedSharedAgent,
    );
    assert!(fixed.config().validate_partition_admission().is_ok());
    let mut bad = Fixture::new(BridgePlatform::Telegram, BridgePartitionPolicy::default());
    bad.config_mut().partition = BridgePartitionPolicy::FixedSharedAgent;
    assert!(bad.config().validate().is_ok());
    assert!(bad.config().validate_partition_admission().is_err());
    for admission in [
        BridgeAdmissionPolicy::All,
        BridgeAdmissionPolicy::Allowlist {
            sender_ids: vec!["sender-one".into()],
        },
    ] {
        let mut fixture = Fixture::new(BridgePlatform::Telegram, BridgePartitionPolicy::default());
        fixture.config_mut().admission = admission;
        assert!(matches!(
            fixture.decision(fixture.facts()),
            PartitionAdmissionDecision::Admitted { .. }
        ));
        assert_eq!(
            fixture.decision(PlatformCapabilityObservation::unknown(&fixture.inbox).unwrap()),
            refused(PartitionRefusal::UnknownSender)
        );
        fixture.inbox.admission = InboxAdmission::SenderDenied;
        assert_eq!(
            fixture.decision(fixture.facts()),
            refused(PartitionRefusal::UpstreamDenied)
        );
    }
}

#[test]
fn configuration_gates_are_bounded_strict_and_round_trip_without_defaulting_to_open() {
    let scoped = BridgeAdmissionPolicy::Scoped {
        policy: ChannelAdmissionPolicy::default(),
    };
    let encoded = serde_json::to_value(&scoped).unwrap();
    assert_eq!(
        encoded,
        serde_json::json!({"kind":"scoped","policy":{"senders":{"kind":"deny"},"chats":{"kind":"deny"},"groups":"deny"}})
    );
    assert_eq!(scoped, serde_json::from_value(encoded).unwrap());
    for ids in [
        vec![],
        vec!["same".into(), "same".into()],
        vec!["\n".into()],
        vec!["x".repeat(257)],
        (0..513).map(|i| i.to_string()).collect(),
    ] {
        assert!(IdentityAdmission::Allowlist { ids }.validate().is_err());
    }
    assert!(IdentityAdmission::Allowlist {
        ids: (0..512).map(|i| i.to_string()).collect()
    }
    .validate()
    .is_ok());
    assert!(serde_json::from_value::<BridgeAdmissionPolicy>(serde_json::json!({"kind":"scoped","policy":{"senders":{"kind":"open"},"chats":{"kind":"open"}}})).is_err());
    assert!(serde_json::from_value::<ChannelAdmissionPolicy>(serde_json::json!({"senders":{"kind":"open"},"chats":{"kind":"open"},"groups":"all","allow_bots":true})).is_err());
    let mut fixture = Fixture::new(BridgePlatform::Telegram, BridgePartitionPolicy::default());
    fixture.config_mut().admission = BridgeAdmissionPolicy::Scoped {
        policy: ChannelAdmissionPolicy {
            senders: IdentityAdmission::Allowlist {
                ids: (0..100)
                    .map(|i| format!("{i:03}{}", "a".repeat(200)))
                    .collect(),
            },
            chats: IdentityAdmission::Open,
            groups: GroupAdmission::All,
        },
    };
    assert!(matches!(
        fixture.config().validate(),
        Err(BridgeError::InvalidConfiguration("configuration_size"))
    ));
}
