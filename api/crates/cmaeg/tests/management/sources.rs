//! Attach/detach under version control without changing provider bindings.
use super::source_support::*;
use cmaeg::{auth::Creator, channels::management::ManagementError};
use cmaeg_core::channel::Extensions;

#[tokio::test]
async fn each_source_slot_attaches_then_detaches_down_to_zero() {
    let gateway = start().await;
    let (id, record, _) = gateway.seed(|raw| {
        raw["sources"] = json!([]);
        raw["reply"]["sinks"] = json!([]);
        raw["partition"]["key"] = "fixed".into();
    });
    let mut version = 1;
    let mut source_ids = Vec::new();
    let mut entries = vec![routing_fixture::telegram()];
    for platform in ["telegram", "lark", "feishu", "discord", "slack", "whatsapp"] {
        entries.push(relay(platform, &uuid::Uuid::new_v4().to_string()));
    }
    entries.push(custom("key-1"));
    for source in entries {
        let (loaded, source_id) = attach(&gateway, &id, request(version, source, json!({}), None))
            .await
            .unwrap();
        version += 1;
        assert_eq!(loaded.record.version, version);
        source_ids.push(source_id);
        assert_eq!(loaded.definition.sources.len(), source_ids.len());
    }
    assert_eq!(source_ids.len(), 8);
    invalid(
        attach(
            &gateway,
            &id,
            request(version, routing_fixture::telegram(), json!({}), None),
        )
        .await,
        "/sources",
        "must contain at most eight sources",
    );
    for source in source_ids {
        let loaded = sources::detach(
            &gateway.state,
            &creator(),
            &ChannelTarget::Record(record.clone()),
            &source,
            version,
        )
        .await
        .unwrap();
        version += 1;
        assert_eq!(loaded.record.version, version);
        assert!(loaded
            .definition
            .sources
            .iter()
            .all(|value| value.source_id() != source));
    }
    let (stored, actual) = gateway.stored(&record);
    assert!(stored.sources.is_empty());
    assert!(stored.secrets.channel_key.is_none());
    assert!(stored.secrets.telegram_bot_token.is_none());
    assert!(stored.secrets.telegram_webhook_secret.is_none());
    assert!(stored.secrets.hmac_secrets.is_empty());
    assert_eq!(actual, 17);
    assert_eq!(
        gateway.index.changed.lock().unwrap().as_slice(),
        &(2..=17).collect::<Vec<_>>()
    );
    assert_eq!(gateway.calls(), 0);
    gateway.close().await;
}

#[tokio::test]
async fn ownership_versions_flags_and_connection_ownership_precede_source_details() {
    let mut gateway = start().await;
    let (id, record, _) = gateway.seed(|_| {});
    let target = ChannelTarget::Record(record.clone());
    let foreign = Creator {
        subject: "bob".into(),
        token_hash: "fixture".into(),
        organization: false,
    };
    assert!(matches!(
        sources::attach(
            &gateway.state,
            &foreign,
            &target,
            request(1, custom("missing"), json!({}), None)
        )
        .await,
        Err(ManagementError::NotFound)
    ));
    assert!(matches!(
        sources::detach_named(&gateway.state, &foreign, &target, "unknown", 1).await,
        Err(ManagementError::NotFound)
    ));
    assert!(matches!(
        detach(&gateway, &id, "unknown", 2).await,
        Err(ManagementError::Conflict { .. })
    ));
    gateway.state.routing.dynamic_sources_enabled = false;
    for error in [
        attach(
            &gateway,
            &id,
            request(1, custom("missing"), json!({}), None),
        )
        .await
        .unwrap_err(),
        detach(&gateway, &id, "unknown", 1).await.unwrap_err(),
    ] {
        assert!(matches!(
            error,
            ManagementError::InvalidReason("dynamic_sources_disabled")
        ));
    }
    gateway.state.routing.dynamic_sources_enabled = true;
    for name in ["unknown", "agent", "custom", "relay_feishu"] {
        assert!(matches!(
            detach(&gateway, &id, name, 1).await,
            Err(ManagementError::SourceNotFound)
        ));
    }
    let (connected, _, _) = gateway.seed(|raw| raw["connection_ref"] = "conn_fixture".into());
    for error in [
        attach(
            &gateway,
            &connected,
            request(1, custom("key-1"), json!({}), Some("fixed")),
        )
        .await
        .unwrap_err(),
        detach(&gateway, &connected, "relay_lark", 1)
            .await
            .unwrap_err(),
    ] {
        assert!(matches!(
            error,
            ManagementError::InvalidReason("source_managed_by_connection")
        ));
    }
    assert_eq!(gateway.stored(&record).1, 1);
    assert!(gateway.index.changed.lock().unwrap().is_empty());
    assert_eq!(gateway.calls(), 0);
    gateway.close().await;
}

#[tokio::test]
async fn two_replica_attaches_at_one_version_have_one_winner_and_invalidate_every_alias() {
    let gateway = start().await;
    let (id, record, _) = gateway.seed(|_| {});
    let alias = id
        .open(&gateway.state.master_keys)
        .unwrap()
        .payload
        .seal_id(&gateway.state.master_keys)
        .unwrap();
    gateway.state.resolver.resolve(id.as_str()).await.unwrap();
    gateway
        .state
        .resolver
        .resolve(alias.as_str())
        .await
        .unwrap();
    let mut other = ManagementState::new(
        gateway.state.providers.clone(),
        gateway.state.records.clone(),
        gateway.state.creator_auth.clone(),
        cmaeg::ManagementConfig {
            master_keys: gateway.state.master_keys.clone(),
            public_url: gateway.state.public_url.clone(),
            cma_base_url: gateway.state.cma_base_url.clone(),
            tools_nyxid_slug: gateway.state.tools_nyxid_slug.clone(),
            cma_service_token: gateway.state.cma_service_token.clone(),
            nyxid_base_url: "http://127.0.0.1:1".into(),
        },
        Default::default(),
    );
    other.routing = gateway.state.routing.clone();
    other.resolver = gateway.state.resolver.clone();
    other.index = gateway.index.clone();
    let first = request(
        1,
        relay("feishu", &uuid::Uuid::new_v4().to_string()),
        json!({}),
        None,
    );
    let second = request(
        1,
        relay("slack", &uuid::Uuid::new_v4().to_string()),
        json!({}),
        None,
    );
    let target = ChannelTarget::Record(record.clone());
    let creator = creator();
    let (a, b) = tokio::join!(
        sources::attach(&gateway.state, &creator, &target, first),
        sources::attach(&other, &creator, &target, second)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let error = a.err().or_else(|| b.err()).unwrap();
    assert!(matches!(
        error,
        ManagementError::Conflict { .. }
            | ManagementError::Record(cmaeg::records::RecordError::Conflict { .. })
    ));
    assert_eq!(gateway.state.resolver.cache_len(), 0);
    for channel in [&id, &alias] {
        let current = gateway
            .state
            .resolver
            .resolve(channel.as_str())
            .await
            .unwrap();
        assert_eq!(current.version, 2);
        assert_eq!(current.definition.sources.len(), 2);
    }
    assert_eq!(gateway.index.changed.lock().unwrap().as_slice(), [2]);
    assert_eq!(gateway.calls(), 0);
    gateway.close().await;
}

#[tokio::test]
async fn source_only_mutations_do_not_require_an_agent_credential_and_return_to_legacy_shape() {
    let gateway = start().await;
    let (id, record, original) = gateway.seed(|raw| {
        raw.as_object_mut().unwrap().remove("routing");
        raw["secrets"]
            .as_object_mut()
            .unwrap()
            .remove("consumer_keys");
        raw["secrets"]["provider_credential"] = Value::Null;
    });
    assert!(Extensions::of(&original).legacy_compatible());
    attach(
        &gateway,
        &id,
        request(
            1,
            relay("feishu", &uuid::Uuid::new_v4().to_string()),
            json!({}),
            None,
        ),
    )
    .await
    .unwrap();
    let detached = detach(&gateway, &id, "relay_feishu", 2).await.unwrap();
    assert!(Extensions::of(&detached.definition).legacy_compatible());
    assert_eq!(detached.definition.sources, original.sources);
    assert!(detached.definition.secrets.provider_credential.is_none());
    let blob = &gateway.cma.records()[record.as_str()].blob;
    let bytes = cmaeg_core::seal::open_blob(&key_ring(1), blob)
        .unwrap()
        .payload;
    serde_json::from_slice::<previous_schema::ChannelDefinition>(&bytes).unwrap();
    assert_eq!(gateway.stored(&record).1, 3);
    assert_eq!(gateway.calls(), 0);
    gateway.close().await;
}
