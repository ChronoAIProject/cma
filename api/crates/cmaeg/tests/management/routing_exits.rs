use super::*;

#[tokio::test]
async fn routing_cascade_unbinds_pending_consumers_and_restores_the_legacy_shape_with_flag_off() {
    let app = Gateway::start(false, 4, 404).await;
    let (id, record, before) = app.seed(|value| {
        value["routing"]["consumers"][1]["pending"] = true.into();
    });
    problem(
        app.put(&id, &body(1)).await,
        400,
        "invalid_request",
        "multi_consumer_disabled",
    )
    .await;
    problem(
        app.remove(&id, 1, false).await,
        409,
        "conflict",
        "consumers_present",
    )
    .await;
    assert_eq!(app.calls(), 0);
    assert!(app.index.0.lock().unwrap().is_empty());
    app.state.resolver.resolve(id.as_str()).await.unwrap();
    let response = app.remove(&id, 1, true).await;
    assert_eq!(response.status(), 200);
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["version"], 2);
    assert!(value["definition"].get("routing").is_none());
    let (after, bytes, version) = app.stored(&record);
    assert_eq!(version, 2);
    assert!(after.routing.is_none());
    assert!(after.secrets.consumer_keys.is_empty());
    assert!(after.partition.custom_key.is_none());
    assert_eq!(after.sources, before.sources);
    assert!(Extensions::of(&after).legacy_compatible());
    serde_json::from_slice::<previous_schema::ChannelDefinition>(&bytes).unwrap();
    for (provider, binding, consumer) in [
        (&app.provider, "binding-reviewer", fixture::REVIEWER),
        (&app.other, "binding-nyx", fixture::NYX),
    ] {
        let calls = provider.requests();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "DELETE");
        assert!(calls[0].path.ends_with(binding));
        assert_eq!(
            calls[0].authorization.as_deref(),
            Some(format!("Bearer {}", fixture::agent_key(consumer)).as_str())
        );
    }
    assert_eq!(app.state.resolver.cache_len(), 0);
    assert_eq!(
        *app.index.0.lock().unwrap(),
        [(record.to_string(), "alice".into(), 2)]
    );
    app.close().await;
}

#[tokio::test]
async fn routing_removal_preserves_source_extensions_and_only_strips_an_inert_key() {
    for enabled in [false, true] {
        for empty in [false, true] {
            let app = Gateway::start(enabled, 4, 204).await;
            let (id, record, before) = app.seed(|value| {
                value["routing"]["consumers"] = json!([]);
                value["secrets"]
                    .as_object_mut()
                    .unwrap()
                    .remove("consumer_keys");
                value["sources"] = if empty {
                    json!([])
                } else {
                    json!([
                        fixture::relay("lark", fixture::ROUTE),
                        fixture::relay("feishu", "f9b20438-cdea-4dbd-bb6b-5e983bd78a69")
                    ])
                };
                value["reply"]["sinks"] = json!([{"type":"sync"}]);
            });
            let response = app.remove(&id, 1, false).await;
            assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
            let (after, _, version) = app.stored(&record);
            assert_eq!(version, 2);
            assert!(after.routing.is_none());
            assert!(after.partition.custom_key.is_none());
            assert_eq!(after.sources, before.sources);
            assert_eq!(after.secrets, before.secrets);
            assert_eq!(after.partition.key, before.partition.key);
            assert!(Extensions::of(&after).sources);
            assert_eq!(app.calls(), 0);
            app.close().await;
        }
    }
}

#[tokio::test]
async fn routing_removal_keeps_a_custom_key_that_is_in_use() {
    let app = Gateway::start(false, 4, 204).await;
    let (id, record, before) = app.seed(|value| {
        value["sources"]
            .as_array_mut()
            .unwrap()
            .push(fixture::custom());
        value["partition"]["key"] = "conversation".into();
        value["routing"]["consumers"] = json!([]);
        value["secrets"]
            .as_object_mut()
            .unwrap()
            .remove("consumer_keys");
    });
    assert_eq!(app.remove(&id, 1, false).await.status(), 200);
    let (after, _, _) = app.stored(&record);
    assert_eq!(after.partition, before.partition);
    assert_eq!(after.sources, before.sources);
    assert_eq!(after.secrets, before.secrets);
    assert_eq!(app.calls(), 0);
    app.close().await;
}

#[tokio::test]
async fn failed_cascade_keeps_the_record_and_notifies_no_observer() {
    let app = Gateway::start(true, 4, 500).await;
    let (id, record, _) = app.seed(|_| {});
    let before = app.cma.records()[record.as_str()].clone();
    for _ in 0..2 {
        let response = app.remove(&id, 1, true).await;
        assert_eq!(response.status(), 503);
        let after = app.cma.records()[record.as_str()].clone();
        assert_eq!(after.version, before.version);
        assert_eq!(after.blob, before.blob);
        assert!(app.index.0.lock().unwrap().is_empty());
    }
    assert_eq!(app.provider.requests().len(), 2);
    assert_eq!(app.other.requests().len(), 2);
    app.close().await;
}

#[tokio::test]
async fn routing_cas_conflicts_do_not_publish_a_new_index_head() {
    let app = Gateway::start(true, 4, 204).await;
    let (id, record, _) = app.seed(|_| {});
    let before = app.cma.records()[record.as_str()].clone();
    app.cma
        .put_conflicts
        .store(1, std::sync::atomic::Ordering::SeqCst);
    let response = app.put(&id, &body(1)).await;
    assert_eq!(response.status(), 409);
    let after = app.cma.records()[record.as_str()].clone();
    assert_eq!(after.blob, before.blob);
    // The fake models a concurrent writer by advancing only the version.
    assert_eq!(after.version, before.version + 1);
    assert!(app.index.0.lock().unwrap().is_empty());
    assert_eq!(app.calls(), 0);
    app.close().await;
}

#[tokio::test]
async fn an_omitted_max_targets_still_checks_the_configured_cap() {
    let app = Gateway::start(true, 2, 204).await;
    let (id, record, _) = app.seed(|_| {});
    problem(
        app.put(&id, &body(1)).await,
        400,
        "invalid_request",
        "/max_targets: max_targets exceeds the routed channel turn cap (2)",
    )
    .await;
    assert_eq!(app.stored(&record).2, 1);
    assert!(app.index.0.lock().unwrap().is_empty());
    app.close().await;
}

#[tokio::test]
async fn deleting_absent_routing_preserves_source_extensions_with_the_flag_off() {
    for empty in [false, true] {
        let app = Gateway::start(false, 4, 204).await;
        let (id, record, before) = app.seed(|value| {
            value.as_object_mut().unwrap().remove("routing");
            value["secrets"]
                .as_object_mut()
                .unwrap()
                .remove("consumer_keys");
            value["sources"] = if empty {
                json!([])
            } else {
                json!([
                    fixture::relay("lark", fixture::ROUTE),
                    fixture::relay("feishu", "f9b20438-cdea-4dbd-bb6b-5e983bd78a69")
                ])
            };
            value["reply"]["sinks"] = json!([{"type":"sync"}]);
        });
        assert!(before.partition.custom_key.is_some());
        assert_eq!(app.remove(&id, 1, false).await.status(), 200);
        let (after, _, version) = app.stored(&record);
        let mut expected = before;
        expected.version = 1;
        assert_eq!(after, expected);
        assert_eq!(version, 2);
        assert_eq!(app.calls(), 0);
        assert_eq!(
            *app.index.0.lock().unwrap(),
            [(record.to_string(), "alice".into(), 2)]
        );
        app.close().await;
    }
}
