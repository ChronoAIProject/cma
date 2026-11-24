use super::*;

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set"]
async fn populated_canary_receipt_roundtrips_with_only_its_own_sandbox() {
    let (temp, store, request, bytes) = fixture().await;
    let operations = store
        .database
        .collection::<Document>("agent_lifecycle_operations");
    let own = doc! {
        "_id": "owned-operation",
        "owner": "owner",
        "sandbox_id": &request.sandbox_id,
        "agent_id": null,
        "document_schema_version": 1_i64,
        "canary_admission": {
            "grant_id": "owned-grant",
            "policy_revision": 3_i64,
            "consent_sha256": "a".repeat(64),
            "release": {
                "release_key": "default",
                "image": {"uri": format!("registry.example/runtime@sha256:{}", "8".repeat(64))}
            }
        }
    };
    let mut foreign = own.clone();
    foreign.insert("_id", "foreign-operation");
    foreign.insert("owner", "other-owner");
    foreign.insert("sandbox_id", "sbx_other");
    foreign
        .get_document_mut("canary_admission")
        .unwrap()
        .insert("grant_id", "foreign-grant");
    operations
        .insert_many([own.clone(), foreign.clone()])
        .await
        .unwrap();
    let custody = Custody::default();
    store.admit(&request, &bytes).await.unwrap();
    let receipt = export::publish(&store, &request, &bytes, &custody, None)
        .await
        .unwrap();
    let restore = temp.0.join("restore");
    verify::restore(&custody, &receipt, &request, &bytes, &restore)
        .await
        .unwrap();
    let restored = std::fs::read(restore.join("database/agent_lifecycle_operations.bson")).unwrap();
    assert_eq!(restored, mongodb::bson::serialize_to_vec(&own).unwrap());
    assert_eq!(
        operations
            .find_one(doc! {"_id":"foreign-operation"})
            .await
            .unwrap(),
        Some(foreign)
    );
    store.database.drop().await.unwrap();
}
