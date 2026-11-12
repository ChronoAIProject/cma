//! Global counters are never part of one Sandbox's retained accounting.
use super::*;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn platform_counter_keys_allow_capture_without_incidental_owner_selection() {
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
    for start in [0_i64, 1_767_225_600, i64::MAX] {
        for shard in 0..16 {
            rows.insert_one(doc! {
                "_id": format!("platform-counter:{start}:{shard}"),
                "document_schema_version": 1_i64,
                "record": {"period_start": start, "shard": shard,
                    "spent": 0_i64, "reserved": 0_i64, "revision": 1_i64,
                    "agent_id": record.requester.agent_id.as_str(),
                    "hold": {"scope": {"agent_id": record.requester.agent_id.as_str()}}}
            })
            .await
            .unwrap();
        }
    }
    support::roundtrip(
        fixture,
        std::collections::BTreeMap::from([("model_budget_reservations".into(), Vec::new())]),
    )
    .await;
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn malformed_platform_counter_keys_refuse_without_releasing_the_hold() {
    let (_temp, store, request, bytes) = fixture().await;
    store.admit(&request, &bytes).await.unwrap();
    let held = store.inspect(&request.sandbox_id).await.unwrap();
    let rows = store
        .database
        .collection::<Document>("model_budget_reservations");
    for key in [
        "platform-counter:",
        "platform-counter::0",
        "platform-counter:1",
        "platform-counter:-1:0",
        "platform-counter:+1:0",
        "platform-counter:01:0",
        "platform-counter:1.0:0",
        "platform-counter:9223372036854775808:0",
        "platform-counter:18446744073709551616:0",
        "platform-counter:1:-1",
        "platform-counter:1:+1",
        "platform-counter:1:01",
        "platform-counter:1:16",
        "platform-counter:1:1.0",
        "platform-counter:1:0:extra",
        "platform-counter:1:0\n",
        "other-platform-counter:1:0",
    ] {
        rows.insert_one(doc! {"_id": key, "document_schema_version": 1_i64, "record": {}})
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
        rows.delete_one(doc! {"_id": key}).await.unwrap();
    }
    store.database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn platform_counter_bindings_remain_byte_exact_in_reservation_archives() {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("archive_counter_bindings_{}", uuid::Uuid::new_v4().simple());
    let (ordinary, scope, model) = crate::store::sandbox_mongo_integration_tests::command_ledger_tests::execution_tests::request_budget::completed(&client, &name).await;
    ordinary
        .reserve_request(&scope, &model)
        .await
        .unwrap()
        .unwrap();
    let agent = ordinary.get_agent(&scope.agent_id).await.unwrap().unwrap();
    let database = client.database(&name);
    // Archive the strict reader extension opaquely: restoration must retain
    // original accounting bindings, including partition zero for old holds.
    let changed = database
        .collection::<Document>("model_budget_reservations")
        .update_many(
            doc! {"_id": {"$regex": r"\A(?:command|turn|request):"}},
            doc! {"$set": {"record.period_start": 1_767_225_600_i64, "record.period_shard": 0_i32}},
        )
        .await
        .unwrap();
    assert_eq!(changed.matched_count, 3);
    let fixture =
        support::prepare_source(client, database.clone(), agent.sandbox_id.as_str()).await;
    let expected = support::inventory(&database, &["model_budget_reservations"]).await;
    support::roundtrip(fixture, expected).await;
}
