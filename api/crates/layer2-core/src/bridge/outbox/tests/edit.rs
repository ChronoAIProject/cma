use super::*;

fn edit_scope(platform: BridgePlatform) -> ChannelReplyTurnAuthorization {
    let mut scope = scope();
    scope.source.config.connector.platform = platform;
    scope
}
fn original(platform: BridgePlatform) -> OutboxItem {
    let intent = ChannelReplyToolIntent::prepare(
        edit_scope(platform),
        ChannelReplyToolRequest {
            request_key: "initial".into(),
            text: "Initial progress".into(),
        },
    )
    .unwrap();
    let mut original = OutboxItem::from_tool_intent(&intent, key(), 1, None, 100)
        .unwrap()
        .remove(0);
    send(&mut original, DeliveryOutcome::Confirmed(receipt()));
    original
}
fn edit_intent(platform: BridgePlatform, id: &str, text: &str) -> ChannelReplyToolIntent {
    ChannelReplyToolIntent::prepare_edit(
        edit_scope(platform),
        ChannelReplyEditRequest {
            original_outbox_id: id.into(),
            request_key: "edit".into(),
            text: text.into(),
        },
    )
    .unwrap()
}

#[test]
fn explicit_lark_and_feishu_edits_retain_confirmed_source_and_one_destination() {
    for platform in [BridgePlatform::Lark, BridgePlatform::Feishu] {
        let original = original(platform.clone());
        let intent = edit_intent(platform, &original.id, "Updated progress");
        let edits = OutboxItem::from_edit_intent(
            &intent,
            &original,
            key(),
            2,
            Some(original.id.clone()),
            104,
        )
        .unwrap();
        assert_eq!(edits.len(), 1);
        let edit = &edits[0];
        assert_eq!(
            edit.source.fingerprint().unwrap(),
            original.source.fingerprint().unwrap()
        );
        assert_eq!(edit.require_edit_original(&original).unwrap(), &receipt());
        assert_eq!(
            edit.operation,
            RelayOperation::Edit {
                original_outbox_id: original.id.clone(),
                platform_message_id: "platform-reply".into()
            }
        );
        assert_eq!(edit.text(), "Updated progress");
        assert!(
            OutboxItem::from_tool_intent(&intent, key(), 2, Some(original.id.clone()), 104)
                .is_err()
        );
    }
}

#[test]
fn edits_reject_unconfirmed_foreign_source_wrong_target_and_oversized_replacements() {
    let original = original(BridgePlatform::Lark);
    let intent = edit_intent(BridgePlatform::Lark, &original.id, "Updated");
    for state in [
        DeliveryState::Pending,
        DeliveryState::Rejected {
            reason: DeliveryFailure::ProviderFailure,
        },
    ] {
        let mut unsent = original.clone();
        unsent.state = state;
        unsent.attempts.clear();
        assert!(OutboxItem::from_edit_intent(
            &intent,
            &unsent,
            key(),
            2,
            Some(original.id.clone()),
            104
        )
        .is_err());
    }
    let mut foreign = edit_scope(BridgePlatform::Lark);
    foreign.source.chat_id = "another-chat".into();
    let request = ChannelReplyEditRequest {
        original_outbox_id: original.id.clone(),
        request_key: "foreign".into(),
        text: "Updated".into(),
    };
    let foreign = ChannelReplyToolIntent::prepare_edit(foreign, request).unwrap();
    assert!(OutboxItem::from_edit_intent(
        &foreign,
        &original,
        key(),
        2,
        Some(original.id.clone()),
        104
    )
    .is_err());
    let wrong = edit_intent(
        BridgePlatform::Lark,
        &format!("bro_{}", "a".repeat(64)),
        "Updated",
    );
    assert!(OutboxItem::from_edit_intent(
        &wrong,
        &original,
        key(),
        2,
        Some(original.id.clone()),
        104
    )
    .is_err());
    let large = edit_intent(BridgePlatform::Lark, &original.id, &"a".repeat(1901));
    assert!(matches!(
        OutboxItem::from_edit_intent(&large, &original, key(), 2, Some(original.id.clone()), 104),
        Err(OutboxError::Bounds)
    ));
    let edits =
        OutboxItem::from_edit_intent(&intent, &original, key(), 2, Some(original.id.clone()), 104)
            .unwrap();
    let edit_an_edit = edit_intent(BridgePlatform::Lark, &edits[0].id, "Another");
    assert!(OutboxItem::from_edit_intent(
        &edit_an_edit,
        &edits[0],
        key(),
        3,
        Some(edits[0].id.clone()),
        105
    )
    .is_err());
}

#[test]
fn edit_identity_retains_key_namespace_and_binds_operation_target_and_text() {
    let original = original(BridgePlatform::Lark);
    let edit = edit_intent(BridgePlatform::Lark, &original.id, "Updated");
    let plain = ChannelReplyToolIntent::prepare(
        edit_scope(BridgePlatform::Lark),
        ChannelReplyToolRequest {
            request_key: "edit".into(),
            text: "Updated".into(),
        },
    )
    .unwrap();
    assert_eq!(edit.id(), plain.id());
    assert_ne!(edit.request_sha256(), plain.request_sha256());
    assert_ne!(
        edit.request_sha256(),
        edit_intent(BridgePlatform::Lark, &original.id, "Changed").request_sha256()
    );
    assert_ne!(
        edit.request_sha256(),
        edit_intent(
            BridgePlatform::Lark,
            &format!("bro_{}", "a".repeat(64)),
            "Updated"
        )
        .request_sha256()
    );
    assert!(matches!(
        ChannelReplyToolIntent::prepare_edit(
            scope(),
            ChannelReplyEditRequest {
                original_outbox_id: original.id,
                request_key: "edit".into(),
                text: "Updated".into()
            }
        ),
        Err(OutboxError::CapabilityUnavailable)
    ));
}
