use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn observer_refuses_indeterminate_leases_and_accepts_valid_expiry_on_both_clocks() {
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_activity_desktop").await;
    let expiry = std::time::SystemTime::now() - Duration::from_secs(90);
    let displayed = layer2_core::rfc3339_at(expiry);
    let expires_ms =
        i64::try_from(layer2_core::seconds_from_rfc3339(&displayed).unwrap()).unwrap() * 1000;
    let legacy = doc! {
        "_id": "fixture-desktop", "sandbox_id": sandbox.id.as_str(),
        "lease_expires_at": displayed.as_str(),
    };
    let mut invalid_legacy = legacy.clone();
    invalid_legacy.insert("unexpected", true);
    let mut exclusive = legacy;
    exclusive.extend(doc! {
        "document_schema_version": crate::store::sandbox_mongo::DOCUMENT_SCHEMA_VERSION,
        "runtime_id": "fixture-runtime", "boot_id": mongodb::bson::Bson::Null, "epoch": 1_i64,
        "issued_at_ms": expires_ms - 10_000, "expires_at_ms": expires_ms + 1000,
    });
    for invalid in [invalid_legacy, exclusive.clone()] {
        store
            .sandbox_sessions
            .replace_one(doc! {"_id": "fixture-desktop"}, invalid)
            .upsert(true)
            .await
            .unwrap();
        assert!(matches!(
            store
                .observe_runtime_activity(&sandbox.id, &context(&store))
                .await
                .unwrap(),
            RuntimeActivityObservation::Unknown(RuntimeActivityUnknownReason::IncompleteCoverage)
        ));
    }
    exclusive.insert("expires_at_ms", expires_ms);
    store
        .sandbox_sessions
        .replace_one(doc! {"_id": "fixture-desktop"}, exclusive)
        .await
        .unwrap();
    let before = observe(&store, &sandbox.id).await;
    assert!(!before.has_activity());
    assert_eq!(before.vector.desktop_leases.len(), 1);
    // Mongo's read-time clock must never enter the retained metadata digest.
    assert_eq!(observe(&store, &sandbox.id).await.vector, before.vector);
    client.database(&name).drop().await.unwrap();
}
