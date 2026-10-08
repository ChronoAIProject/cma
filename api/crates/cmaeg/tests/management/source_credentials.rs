//! Attach validation and detach pruning preserve only actual secret owners.
use super::source_support::*;

#[tokio::test]
async fn relay_identity_routes_and_retained_channel_key_are_shared() {
    let gateway = start().await;
    let (id, record, _) = gateway.seed(|_| {});
    for (source, credentials, pointer, message) in [
        (
            relay("feishu", routing_fixture::ROUTE),
            json!({}),
            "/source/route_ids",
            "route id is already used by another relay source",
        ),
        (
            {
                let mut source = relay("feishu", &uuid::Uuid::new_v4().to_string());
                source["key_id"] = "other".into();
                source
            },
            json!({}),
            "/source/key_id",
            "relay sources must share issuer, jwks_url and key_id",
        ),
        (
            relay("feishu", &uuid::Uuid::new_v4().to_string()),
            json!({"channel_key":"different-fixture-key"}),
            "/credentials/channel_key",
            "relay sources share the channel key",
        ),
        (
            custom("key-1"),
            json!({"channel_key":"different-fixture-key"}),
            "/credentials/channel_key",
            "relay sources share the channel key",
        ),
    ] {
        let partition = (source["type"] == "custom").then_some("fixed");
        invalid(
            attach(&gateway, &id, request(1, source, credentials, partition)).await,
            pointer,
            message,
        );
    }
    assert_eq!(gateway.stored(&record).1, 1);
    let (loaded, _) = attach(
        &gateway,
        &id,
        request(
            1,
            relay("feishu", &uuid::Uuid::new_v4().to_string()),
            json!({"channel_key":"fixture-channel-key"}),
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(
        loaded.definition.secrets.channel_key.unwrap().as_str(),
        "fixture-channel-key"
    );
    let one = detach(&gateway, &id, "relay_lark", 2).await.unwrap();
    assert!(one.definition.secrets.channel_key.is_some());
    gateway
        .replace(&record, |definition| definition.reply.sinks.clear())
        .await;
    let empty = detach(&gateway, &id, "relay_feishu", 4).await.unwrap();
    assert!(empty.definition.secrets.channel_key.is_none());
    assert_eq!(gateway.calls(), 0);
    gateway.close().await;
}

#[tokio::test]
async fn supplied_credentials_and_new_source_secret_requirements_are_validated() {
    let gateway = start().await;
    let (id, record, _) = gateway.seed(|raw| {
        raw["sources"] = json!([]);
        raw["reply"]["sinks"] = json!([]);
        raw["secrets"]["channel_key"] = Value::Null;
        raw["secrets"]["hmac_secrets"] = json!([]);
        raw["partition"]["key"] = "fixed".into();
    });
    for (source, credentials, pointer, message) in [
        (
            relay("lark", routing_fixture::ROUTE),
            json!({}),
            "/credentials/channel_key",
            "is required for a relay source",
        ),
        (
            custom("new"),
            json!({}),
            "/source/auth/key_id",
            "must reference a supplied HMAC secret",
        ),
        (
            relay("lark", routing_fixture::ROUTE),
            json!({"channel_key":"space is forbidden"}),
            "/credentials/channel_key",
            "must contain 1..=16384 printable bearer bytes",
        ),
        (
            custom("new"),
            json!({"hmac_secrets":[{"key_id":"new","secret":"not-base64"}]}),
            "/credentials/hmac_secrets/0/secret",
            "must be base64",
        ),
        (
            routing_fixture::telegram(),
            json!({"telegram_bot_token":"88:fixture"}),
            "/credentials/telegram_bot_token",
            "must match the Telegram bot identity",
        ),
    ] {
        invalid(
            attach(&gateway, &id, request(1, source, credentials, None)).await,
            pointer,
            message,
        );
    }
    let (custom_loaded, _) = attach(
        &gateway,
        &id,
        request(
            1,
            custom("new"),
            json!({"hmac_secrets":[{"key_id":"new","secret":"Zml4dHVyZQ=="}]}),
            None,
        ),
    )
    .await
    .unwrap();
    assert_eq!(custom_loaded.record.version, 2);
    invalid(
        attach(
            &gateway,
            &id,
            request(
                2,
                relay("lark", routing_fixture::ROUTE),
                json!({"hmac_secrets":[{"key_id":"new","secret":"Zml4dHVyZQ=="}]}),
                None,
            ),
        )
        .await,
        "/credentials/hmac_secrets/0/key_id",
        "key id already exists",
    );
    let duplicate = json!({"hmac_secrets":[{"key_id":"fresh","secret":"Zml4dHVyZQ=="},{"key_id":"fresh","secret":"Zml4dHVyZQ=="}]});
    invalid(
        attach(
            &gateway,
            &id,
            request(2, relay("lark", routing_fixture::ROUTE), duplicate, None),
        )
        .await,
        "/credentials/hmac_secrets/1/key_id",
        "key id already exists",
    );
    assert_eq!(gateway.stored(&record).1, 2);
    gateway.close().await;
}

#[tokio::test]
async fn secret_pruning_retains_sink_hmac_and_relay_telegram_credentials() {
    let gateway = start().await;
    let (id,record,_) = gateway.seed(|raw|{
        raw["sources"]=json!([routing_fixture::telegram(),routing_fixture::relay("telegram","route-telegram"),routing_fixture::custom()]);
        raw["partition"]["key"]="conversation".into();raw["partition"]["custom_key"]="event_subject".into();
        raw["reply"]["sinks"]=json!([{"type":"result_webhook","url":"https://receiver.example/events","hmac_key_id":"key-1"}]);
        raw["secrets"]["hmac_secrets"].as_array_mut().unwrap().push(json!(["unowned","Zml4dHVyZQ=="]));
    });
    let native_removed = detach(&gateway, &id, "telegram", 1).await.unwrap();
    assert_eq!(
        native_removed
            .definition
            .secrets
            .telegram_bot_token
            .unwrap()
            .as_str(),
        "77:fixture"
    );
    assert_eq!(
        native_removed
            .definition
            .secrets
            .telegram_webhook_secret
            .unwrap()
            .as_str(),
        "fixture-webhook"
    );
    let custom_removed = detach(&gateway, &id, "custom", 2).await.unwrap();
    assert!(custom_removed.definition.partition.custom_key.is_none());
    assert_eq!(custom_removed.definition.secrets.hmac_secrets.len(), 1);
    assert_eq!(custom_removed.definition.secrets.hmac_secrets[0].0, "key-1");
    let relay_removed = detach(&gateway, &id, "relay_telegram", 3).await.unwrap();
    assert!(relay_removed.definition.sources.is_empty());
    assert!(relay_removed.definition.secrets.channel_key.is_none());
    assert!(relay_removed
        .definition
        .secrets
        .telegram_bot_token
        .is_none());
    assert!(relay_removed
        .definition
        .secrets
        .telegram_webhook_secret
        .is_none());
    assert_eq!(relay_removed.definition.secrets.hmac_secrets.len(), 1);
    assert_eq!(gateway.stored(&record).1, 4);
    gateway.close().await;
}

#[tokio::test]
async fn detaching_a_last_source_does_not_silently_remove_its_source_sink() {
    let gateway = start().await;
    let (id, record, original) = gateway.seed(|_| {});
    invalid(
        detach(&gateway, &id, "relay_lark", 1).await,
        "/reply/sinks",
        "source sink needs a source",
    );
    let (stored, version) = gateway.stored(&record);
    assert_eq!(stored, original);
    assert_eq!(version, 1);
    assert!(gateway.index.changed.lock().unwrap().is_empty());
    gateway.close().await;
}
