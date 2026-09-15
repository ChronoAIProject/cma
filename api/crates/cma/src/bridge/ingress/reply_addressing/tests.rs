use super::*;
use crate::bridge::ingress::capabilities::tests::fixtures::Fixture;
use layer2_core::bridge::outbox::*;
use layer2_core::bridge::{ingress::InboxAdmission, partition::*, BridgeAdmissionPolicy};
use serde_json::json;

fn callback(platform: BridgePlatform) -> Fixture {
    let mut fixture = Fixture::new(platform.clone());
    let parent = match platform {
        BridgePlatform::Telegram => "40",
        _ => "provider-parent",
    };
    fixture.body["reply_to_platform_message_id"] = json!(parent);
    match platform {
        BridgePlatform::Telegram => {
            fixture.raw_mut()["message"]["reply_to_message"] = json!({"message_id":40,"chat":{"id":-100123,"type":"supergroup"},"from":{"id":999,"is_bot":true}})
        }
        BridgePlatform::Discord => {
            fixture.raw_mut()["d"]["type"] = json!(19);
            fixture.raw_mut()["d"]["message_reference"] =
                json!({"type":0,"message_id":parent,"channel_id":"discord-chat"});
        }
        BridgePlatform::Lark | BridgePlatform::Feishu => {
            fixture.raw_mut()["event"]["message"]["parent_id"] = json!(parent)
        }
    }
    fixture
}

fn pending(fixture: &Fixture) -> OutboxItem {
    let bridge = &fixture.bridge;
    let source: OutboxSource = serde_json::from_value(json!({
        "inbox_id":"1".repeat(64),"inbox_sha256":"2".repeat(64),"allocation_id":"allocation",
        "command_id":"command","response_id":"response","admission_sha256":"3".repeat(64),
        "owner":bridge.owner,"pin":bridge.admission_pin().unwrap(),"config":fixture.config(),
        "target":{"sandbox_id":"sandbox","workspace_id":"workspace","worktree_id":"worktree","agent_id":"agent"},
        "message_id":"original-inbound","chat_id":fixture.body["conversation"]["platform_id"],"source_sha256":"4".repeat(64),
        "reply_expires_at_ms":null,"route_expires_at_ms":null
    })).unwrap();
    let invocation = format!("brti_{}", "5".repeat(64));
    let operation = RelayOperation::InitialReply;
    let item_id = format!(
        "bro_{}",
        layer2_core::profiles::content_digest(&(
            "cma.bridge.outbox.v1",
            source.fingerprint().unwrap(),
            &invocation,
            1_u64,
            0_u32,
            &operation
        ))
        .unwrap()
    );
    let now = mongodb::bson::DateTime::now().timestamp_millis();
    serde_json::from_value(json!({
        "id":item_id,"source":source,"tool_invocation_id":invocation,"turn_sequence":1,"sequence":1,"chunk_index":0,"chunk_count":1,"last_chunk":true,
        "predecessor":null,"content_sha256":layer2_core::profiles::content_digest(&"Confirmed reply").unwrap(),"text":"Confirmed reply",
        "credential":{"kind":"retained_key","reference":fixture.config().connector.custody,"revision":1,"key_state_version":3},
        "operation":operation,"revision":1,"state":{"kind":"pending"},"attempts":[],"duplicate_risk":false,"created_at_ms":now,"updated_at_ms":now
    })).unwrap()
}

pub(in crate::bridge::ingress) fn sent(fixture: &Fixture) -> OutboxItem {
    let mut item = pending(fixture);
    let now = item.updated_at_ms;
    let attempt = item
        .claim("fixture".into(), now, now + 30_000, None)
        .unwrap();
    item.begin_send("fixture", &attempt, now).unwrap();
    item.complete(
        &attempt,
        DeliveryOutcome::Confirmed(DeliveryReceipt {
            nyxid_message_id: "nyx-outbound".into(),
            platform_message_id: fixture.body["reply_to_platform_message_id"]
                .as_str()
                .map(str::to_owned),
        }),
        now,
    )
    .unwrap();
    item
}

#[test]
fn signed_reply_to_exact_confirmed_bot_chat_qualifies_on_all_platforms_without_mention_claims() {
    for platform in [
        BridgePlatform::Telegram,
        BridgePlatform::Discord,
        BridgePlatform::Lark,
        BridgePlatform::Feishu,
    ] {
        let mut fixture = callback(platform);
        let item = sent(&fixture);
        for group in [
            GroupAdmission::ReplyToBot,
            GroupAdmission::MentionOrReplyToBot,
            GroupAdmission::Mention,
        ] {
            fixture.config_mut().admission = BridgeAdmissionPolicy::Scoped {
                policy: ChannelAdmissionPolicy {
                    senders: IdentityAdmission::Open,
                    chats: IdentityAdmission::Open,
                    groups: group,
                },
            };
            let signed = fixture.signed().unwrap();
            let body = fixture.bytes();
            let lookup = ReplyLookup::authenticated(&signed.authenticated_payload(&body).unwrap())
                .unwrap()
                .unwrap();
            let proof = lookup.confirm(&item).unwrap().unwrap();
            let write = signed
                .classify_capabilities(&body, &ReplyCapabilities(Some(proof)))
                .unwrap()
                .bind(fixture.binding())
                .unwrap();
            if group == GroupAdmission::Mention {
                assert!(matches!(
                    write.record().admission,
                    InboxAdmission::PartitionDenied {
                        reason: PartitionRefusal::AddressingEvidenceUnavailable
                    }
                ));
            } else {
                assert_eq!(write.record().admission, InboxAdmission::Accepted);
            }
            let stored = serde_json::to_value(write.record()).unwrap();
            assert_eq!(stored["capabilities"]["mentioned_bot"], json!("unknown"));
            assert_eq!(stored["capabilities"]["replied_to_bot"], json!("yes"));
            assert_eq!(
                stored["capabilities"]["reply_receipt"]["item_id"],
                json!(item.id)
            );
            assert!(stored.get("raw_platform_data").is_none());
        }
    }
}

#[test]
fn signed_parent_cross_chat_forward_and_body_substitution_cannot_supply_reply_proof() {
    for platform in [
        BridgePlatform::Telegram,
        BridgePlatform::Discord,
        BridgePlatform::Lark,
        BridgePlatform::Feishu,
    ] {
        let mut fixture = callback(platform);
        let body = fixture.bytes();
        let signed = fixture.signed().unwrap();
        let lookup = ReplyLookup::authenticated(&signed.authenticated_payload(&body).unwrap())
            .unwrap()
            .unwrap();
        let proof = lookup.confirm(&sent(&fixture)).unwrap().unwrap();
        fixture.body["message_id"] = json!("another-callback");
        let changed = fixture.bytes();
        assert!(matches!(
            signed.authenticated_payload(&changed),
            Err(CallbackError::Authentication)
        ));
        assert!(fixture
            .signed()
            .unwrap()
            .classify_capabilities(&changed, &ReplyCapabilities(Some(proof)))
            .is_err());
        fixture.body["reply_to_platform_message_id"] = json!("different-parent");
        assert!(ReplyLookup::authenticated(
            &fixture
                .signed()
                .unwrap()
                .authenticated_payload(&fixture.bytes())
                .unwrap()
        )
        .unwrap()
        .is_none());
    }
    let mut telegram = callback(BridgePlatform::Telegram);
    telegram.raw_mut()["message"]["reply_to_message"]["chat"]["id"] = json!(-100999);
    assert!(ReplyLookup::authenticated(
        &telegram
            .signed()
            .unwrap()
            .authenticated_payload(&telegram.bytes())
            .unwrap()
    )
    .unwrap()
    .is_none());
    let mut discord = callback(BridgePlatform::Discord);
    discord.raw_mut()["d"]["message_reference"]["type"] = json!(1);
    assert!(ReplyLookup::authenticated(
        &discord
            .signed()
            .unwrap()
            .authenticated_payload(&discord.bytes())
            .unwrap()
    )
    .unwrap()
    .is_none());
}

#[test]
fn pending_sending_unknown_and_foreign_owner_bot_chat_receipts_never_prove_addressing() {
    let fixture = callback(BridgePlatform::Telegram);
    let signed = fixture.signed().unwrap();
    let body = fixture.bytes();
    let lookup = ReplyLookup::authenticated(&signed.authenticated_payload(&body).unwrap())
        .unwrap()
        .unwrap();
    let mut item = pending(&fixture);
    assert!(lookup.confirm(&item).unwrap().is_none());
    let now = item.updated_at_ms;
    let attempt = item
        .claim("worker".into(), now, now + 30_000, None)
        .unwrap();
    assert!(lookup.confirm(&item).unwrap().is_none());
    item.begin_send("worker", &attempt, now).unwrap();
    assert!(lookup.confirm(&item).unwrap().is_none());
    item.complete(
        &attempt,
        DeliveryOutcome::Unknown(DeliveryFailure::WorkerLost),
        now,
    )
    .unwrap();
    assert!(lookup.confirm(&item).unwrap().is_none());
    let item = sent(&fixture);
    for field in ["owner", "issuer", "bot", "platform", "chat", "parent"] {
        let mut foreign = lookup.clone();
        match field {
            "owner" => {
                foreign.owner = ResourceOwner::Person {
                    subject: layer2_core::SubjectId::new("foreign-owner"),
                }
            }
            "issuer" => foreign.issuer = "https://foreign.example".into(),
            "bot" => foreign.bot_id = "different-bot".into(),
            "platform" => foreign.platform = BridgePlatform::Feishu,
            "chat" => foreign.chat_id = "another-chat".into(),
            _ => foreign.parent_id = "another-parent".into(),
        }
        assert!(foreign.confirm(&item).unwrap().is_none(), "{field}");
    }
}

pub(crate) fn sent_for_bridge(bridge: layer2_core::bridge::BridgeRecord) -> OutboxItem {
    let mut fixture = callback(BridgePlatform::Telegram);
    fixture.bridge = bridge;
    sent(&fixture)
}
