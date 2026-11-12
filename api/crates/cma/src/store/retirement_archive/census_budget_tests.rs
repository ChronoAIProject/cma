use super::*;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn stop_evidence_roundtrips_exactly_and_excludes_other_agents_and_prefix_neighbours() {
    let (client, name, _, record) =
        crate::store::agent_access_requests::tests::approved_fixture().await;
    let database = client.database(&name);
    let fixture = support::prepare_source(
        client,
        database.clone(),
        record.requester.sandbox_id.as_str(),
    )
    .await;
    let rows = database.collection::<Document>("model_budget_reservations");
    for turn in [1, 42] {
        rows.insert_one(doc! {
            "_id":format!("stop:{}:{turn}", record.requester.agent_id),
            "document_schema_version":1_i64,
            "record":mongodb::bson::serialize_to_bson(&layer2_core::budget::BudgetError::OwnerExhausted).unwrap()
        }).await.unwrap();
    }
    let expected = support::inventory(
        &database,
        &["model_budget_reservations", "agents", "agent_log"],
    )
    .await;
    for agent in [
        "agt_unrelated".to_string(),
        format!("{}_suffix", record.requester.agent_id),
    ] {
        rows.insert_one(doc! {
            "_id":format!("stop:{agent}:1"), "document_schema_version":1_i64,
            "record":"owner_exhausted"
        })
        .await
        .unwrap();
    }
    support::roundtrip(fixture, expected).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn malformed_stop_evidence_refuses_without_releasing_the_hold() {
    let (_temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    let rows = store
        .database
        .collection::<Document>("model_budget_reservations");
    for (key, record) in [
        ("stop:", Bson::String("owner_exhausted".into())),
        ("stop::1", Bson::String("owner_exhausted".into())),
        ("stop:agt_missing", Bson::String("owner_exhausted".into())),
        (
            "stop:agt_missing:invalid",
            Bson::String("owner_exhausted".into()),
        ),
        (
            "stop:agt_missing:-1",
            Bson::String("owner_exhausted".into()),
        ),
        (
            "stop:agt_missing:1:extra",
            Bson::String("owner_exhausted".into()),
        ),
        (
            "stop:agt_missing:1\n",
            Bson::String("owner_exhausted".into()),
        ),
        ("stop:agt_missing:1", Bson::Document(doc! {})),
        (
            "stop:agt_missing:1",
            Bson::Array(vec![Bson::String("owner_exhausted".into())]),
        ),
    ] {
        rows.insert_one(doc! {
            "_id":key, "document_schema_version":1_i64, "record":record
        })
        .await
        .unwrap();
        assert!(
            export::publish(&store, &request, &bytes, &Custody::default(), None)
                .await
                .is_err(),
            "{key:?}"
        );
        assert!(store.manifest(&request, &bytes).await.is_err());
        assert_eq!(store.inspect(&request.sandbox_id).await.unwrap(), held);
        rows.delete_one(doc! {"_id":key}).await.unwrap();
    }
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn platform_accounting_allows_capture_without_entering_the_archive() {
    let fixture = fixture().await;
    let rows = fixture
        .1
        .database
        .collection::<Document>("model_budget_reservations");
    let at = |seconds| {
        layer2_core::rfc3339_at(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
    };
    // Reproduce #1741's writer keys without importing its unmerged types.
    for start in [0_u64, 951_782_400, 1_709_164_800, 1_794_441_600] {
        let through = start + 3_600;
        rows.insert_one(doc! {
            "_id":format!("platform:{}", at(start)), "document_schema_version":1_i64,
            "record":{"period_start":start as i64,"period_end":through as i64,
                "settings_revision":1_i64,"spent":0_i64,"reserved":0_i64,
                "adjustments":0_i64,"cloud":Bson::Null,"closures":[],
                "evaluated_at":start as i64,"revision":1_i64}
        })
        .await
        .unwrap();
        rows.insert_one(doc! {
            "_id":format!("observation:cloud:{}:{}", at(start), at(through)),
            "document_schema_version":1_i64,
            "record":{"observed_through":through as i64,"units":0_i64,
                "recorded_at":through as i64,"settings_revision":1_i64}
        })
        .await
        .unwrap();
    }
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 8);
    let expected =
        std::collections::BTreeMap::from([("model_budget_reservations".into(), Vec::new())]);
    support::roundtrip(fixture, expected).await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn malformed_platform_accounting_keys_refuse_without_releasing_the_hold() {
    let (_temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    let rows = store
        .database
        .collection::<Document>("model_budget_reservations");
    for key in [
        "platform:",
        "platform:arbitrary",
        "platform:2026-11-12",
        "platform:2026-11-12T00:00:00",
        "platform:2026-1-12T00:00:00Z",
        "platform:2026-13-12T00:00:00Z",
        "platform:2026-02-29T00:00:00Z",
        "platform:2024-02-30T00:00:00Z",
        "platform:2100-02-29T00:00:00Z",
        "platform:2026-04-31T00:00:00Z",
        "platform:2026-11-32T00:00:00Z",
        "platform:2026-11-12T24:00:00Z",
        "platform:2026-11-12T00:60:00Z",
        "platform:2026-11-12T00:00:60Z",
        "platform:2026-11-12T00:00:00+00:00",
        "platform:2026-11-12T00:00:00.000Z",
        "platform:2026-11-12T00:00:00Z:extra",
        "platform:2026-11-12T00:00:00Z\n",
        "observation:",
        "observation:cloud:",
        "observation:model_provider:2026-11-12T00:00:00Z:2026-11-12T01:00:00Z",
        "observation:cloud:2026-11-12T00:00:00Z",
        "observation:cloud:arbitrary:2026-11-12T01:00:00Z",
        "observation:cloud:2026-02-29T00:00:00Z:2026-11-12T01:00:00Z",
        "observation:cloud:2026-11-12T00:00:00Z:2026-04-31T01:00:00Z",
        "observation:cloud:2026-11-12T00:00:00Z:arbitrary",
        "observation:cloud:2026-11-12T00:00:00:2026-11-12T01:00:00Z",
        "observation:cloud:2026-11-12T00:00:00Z:2026-11-12T01:00:00",
        "observation:cloud:2026-11-12T00:00:00Z:2026-11-12T01:00:00+00:00",
        "observation:cloud:2026-11-12T00:00:00.000Z:2026-11-12T01:00:00Z",
        "observation:cloud:2026-11-12T00:00:00Z:2026-11-12T01:00:00Z:extra",
        "observation:cloud:2026-11-12T00:00:00Z:2026-11-12T01:00:00Z\n",
    ] {
        rows.insert_one(doc! {
            "_id":key, "document_schema_version":1_i64, "record":{}
        })
        .await
        .unwrap();
        assert!(
            export::publish(&store, &request, &bytes, &Custody::default(), None)
                .await
                .is_err(),
            "{key:?}"
        );
        assert!(store.manifest(&request, &bytes).await.is_err());
        assert_eq!(store.inspect(&request.sandbox_id).await.unwrap(), held);
        rows.delete_one(doc! {"_id":key}).await.unwrap();
    }
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn platform_accounting_ignores_incidental_agent_ownership_fields() {
    let (client, name, _, record) =
        crate::store::agent_access_requests::tests::approved_fixture().await;
    let database = client.database(&name);
    let fixture = support::prepare_source(
        client,
        database.clone(),
        record.requester.sandbox_id.as_str(),
    )
    .await;
    for key in [
        "platform:2026-11-12T00:00:00Z",
        "observation:cloud:2026-11-12T00:00:00Z:2026-11-12T01:00:00Z",
    ] {
        // Independent keys cannot acquire Sandbox ownership through a record field.
        database
            .collection::<Document>("model_budget_reservations")
            .insert_one(doc! {
                "_id":key,"document_schema_version":1_i64,
                "record":{"agent_id":record.requester.agent_id.as_str(),
                    "hold":{"scope":{"agent_id":record.requester.agent_id.as_str()}}}
            })
            .await
            .unwrap();
    }
    let expected =
        std::collections::BTreeMap::from([("model_budget_reservations".into(), Vec::new())]);
    support::roundtrip(fixture, expected).await;
}
