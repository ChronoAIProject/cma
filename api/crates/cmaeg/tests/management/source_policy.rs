//! Source policy refusals are atomic and retained settings survive other edits.
use super::source_support::*;
use base64::Engine as _;
use cmaeg::channels::management::ManagementError;
use cmaeg_core::{channel::PartitionKey, seal::MAX_BLOB_BYTES};

#[tokio::test]
async fn custom_partition_override_is_required_scoped_and_cleared_only_with_custom() {
    let gateway = start().await;
    let (id, record, _) = gateway.seed(|_| {});
    for (source, key, message) in [
        (
            custom("key-1"),
            None,
            "required for a custom source on a chat-keyed channel",
        ),
        (
            custom("key-1"),
            Some("thread"),
            "must be fixed, event_subject, or per_event",
        ),
        (
            routing_fixture::telegram(),
            Some("fixed"),
            "is accepted only for a custom source on a chat-keyed channel",
        ),
    ] {
        invalid(
            attach(&gateway, &id, request(1, source, json!({}), key)).await,
            "/partition_custom_key",
            message,
        );
    }
    assert_eq!(gateway.stored(&record).1, 1);
    let (added, _) = attach(
        &gateway,
        &id,
        request(1, custom("key-1"), json!({}), Some("event_subject")),
    )
    .await
    .unwrap();
    assert_eq!(
        added.definition.partition.custom_key,
        Some(PartitionKey::EventSubject)
    );
    let remaining = detach(&gateway, &id, "relay_lark", 2).await.unwrap();
    assert_eq!(
        remaining.definition.partition.custom_key,
        Some(PartitionKey::EventSubject)
    );
    attach(
        &gateway,
        &id,
        request(
            3,
            routing_fixture::telegram(),
            json!({
                "telegram_bot_token":"77:fixture","telegram_webhook_secret":"fixture-webhook"
            }),
            None,
        ),
    )
    .await
    .unwrap();
    let removed = detach(&gateway, &id, "custom", 4).await.unwrap();
    assert!(removed.definition.partition.custom_key.is_none());
    let (fixed, fixed_record, _) = gateway.seed(|raw| raw["partition"]["key"] = "fixed".into());
    invalid(
        attach(
            &gateway,
            &fixed,
            request(1, custom("key-1"), json!({}), Some("event_subject")),
        )
        .await,
        "/partition_custom_key",
        "is accepted only for a custom source on a chat-keyed channel",
    );
    assert_eq!(gateway.stored(&fixed_record).1, 1);
    gateway.close().await;
}

#[tokio::test]
async fn default_reply_sources_are_protected_and_detached_source_rules_remain_stored() {
    let gateway = start().await;
    for reply in [
        json!({"type":"telegram_chat","source_id":"telegram","chat_id":"42"}),
        json!({"type":"relay_conversation","source_id":"relay_lark","route_id":routing_fixture::ROUTE}),
    ] {
        let (id, record, before) = gateway.seed(|raw| {
            raw["sources"]
                .as_array_mut()
                .unwrap()
                .push(routing_fixture::telegram());
            raw["routing"]["default_reply"] = reply.clone();
        });
        assert!(matches!(
            detach(&gateway, &id, reply["source_id"].as_str().unwrap(), 1).await,
            Err(ManagementError::ConflictReason {
                detail: "default_reply_source_in_use",
                ..
            })
        ));
        assert_eq!(gateway.stored(&record), (before, 1));
    }
    let (id, _, before) = gateway.seed(|raw| {
        raw["sources"]
            .as_array_mut()
            .unwrap()
            .push(routing_fixture::telegram());
        raw["routing"]["rules"] = json!([
            {"id":"only-lark","when":{"sources":["relay_lark"]},"targets":["default"]},
            {"id":"mixed","when":{"sources":["relay_lark","telegram"]},"targets":["default"]}
        ]);
        // Reader-valid inert keys may also exist independently of a custom source.
        raw["partition"]["custom_key"] = "event_subject".into();
    });
    let after = detach(&gateway, &id, "relay_lark", 1)
        .await
        .unwrap()
        .definition;
    let rules = &after.routing.as_ref().unwrap().rules;
    assert_eq!(rules, &before.routing.unwrap().rules);
    assert!(!rules[0].is_active(&after));
    assert!(rules[1].is_active(&after));
    assert_eq!(after.partition.custom_key, Some(PartitionKey::EventSubject));
    gateway.close().await;
}

#[tokio::test]
async fn media_validation_applies_to_the_incoming_source_and_keeps_retained_attachments() {
    let mut gateway = start().await;
    let (id, record, _) = gateway.seed(|_| {});
    let mut source = custom("key-1");
    source["attachments"] = json!({"pointer":"/attachments","origins":["https://files.example"]});
    invalid(
        attach(
            &gateway,
            &id,
            request(1, source.clone(), json!({}), Some("fixed")),
        )
        .await,
        "/source/attachments",
        "custom_attachments_disabled",
    );
    gateway.state.media_enabled = true;
    let mut private = source.clone();
    private["attachments"]["origins"] = json!(["https://10.0.0.1"]);
    invalid(
        attach(&gateway, &id, request(1, private, json!({}), Some("fixed"))).await,
        "/source/attachments/origins/0",
        "attachment_origin_not_public",
    );
    assert_eq!(gateway.stored(&record).1, 1);
    let (added, _) = attach(&gateway, &id, request(1, source, json!({}), Some("fixed")))
        .await
        .unwrap();
    gateway.state.media_enabled = false;
    let (changed, _) = attach(
        &gateway,
        &id,
        request(
            2,
            relay("feishu", &uuid::Uuid::new_v4().to_string()),
            json!({}),
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(changed.definition.sources[1], added.definition.sources[1]);
    let removed = detach(&gateway, &id, "relay_feishu", 3).await.unwrap();
    assert_eq!(removed.definition.sources[1], added.definition.sources[1]);
    assert_eq!(gateway.calls(), 0);
    gateway.close().await;
}

#[tokio::test]
async fn an_oversized_attached_definition_is_refused_before_a_record_write() {
    let gateway = start().await;
    let (id, record, _) = gateway.seed(|raw| {
        // Escaping grows the serialized profile without exceeding its prompt bound.
        raw["profile"]["context"]["system_prompt"] = "\"".repeat(20_000).into();
        raw["profile_digest"] = routing_fixture::digest_of(&raw["profile"]).into();
    });
    let before = gateway.cma.records()[record.as_str()].clone();
    let secret = base64::engine::general_purpose::STANDARD.encode([0_u8; 3072]);
    let credentials = json!({"hmac_secrets":(0..4).map(|i|json!({"key_id":format!("large-{i}"),"secret":secret})).collect::<Vec<_>>()});
    let error = attach(
        &gateway,
        &id,
        request(1, custom("large-0"), credentials, Some("fixed")),
    )
    .await
    .unwrap_err();
    let ManagementError::Invalid(error) = error else {
        panic!("expected size refusal")
    };
    assert_eq!(error.pointer, "/");
    assert!(error
        .message
        .starts_with("sealed channel exceeds the record size limit bytes="));
    assert!(error.message.ends_with(&format!("limit={MAX_BLOB_BYTES}")));
    let after = gateway.cma.records()[record.as_str()].clone();
    assert_eq!((after.version, after.blob), (before.version, before.blob));
    assert!(gateway.index.changed.lock().unwrap().is_empty());
    assert_eq!(gateway.calls(), 0);
    gateway.close().await;
}
