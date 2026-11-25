use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn ownership_retry_discovery_matches_desktop_and_excludes_scoped_migration() {
    let (client, name, store, agent, _, observed) = fixture("retry_desktop_parent").await;
    assert_eq!(observed.write().await, HintWriteResult::Recorded);
    // The fixture captured the hint under a live claim. Both discovery
    // entry points must be able to see the same otherwise eligible Agent.
    store
        .agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$unset": {"record.work.lease": ""}, "$set": {"work_lease_expires_at": Bson::Null}},
        )
        .await
        .unwrap();
    let key = doc! {"_id": agent.sandbox_id.as_str()};
    let now = layer2_core::rfc3339_at(SystemTime::now());
    let runtime = store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap()
        .runtime
        .unwrap()
        .runtime_version;
    store
        .sandboxes
        .update_one(
            key.clone(),
            doc! {"$set": {
                "document_schema_version": 6_i64,
                "origin_type": "lifecycle",
                "origin_operation_id": "desktop-fixture",
                "record.origin": {"type":"lifecycle","operation_id":"desktop-fixture"},
                "record.profile.content.runtime_image": runtime,
                "record.lifecycle.runtime_image_origin": "managed_desktop",
            }},
        )
        .await
        .unwrap();
    let mut parent = store
        .sandboxes
        .find_one(key.clone())
        .await
        .unwrap()
        .unwrap();
    parent.insert("admission_name", doc! {"mode":"allocate","value":"sandbox"});
    parent.insert("admission_fingerprint", "a".repeat(64));
    store
        .sandboxes
        .replace_one(key.clone(), parent.clone())
        .await
        .unwrap();
    for (label, fields, held) in [
        ("no proof", doc! {}, true),
        (
            "valid proof",
            doc! {"record.work.prepared_installation_digest":"a".repeat(64)},
            true,
        ),
        (
            "bad proof",
            doc! {"record.work.prepared_installation_digest":"malformed"},
            false,
        ),
        (
            "scoped migration",
            doc! {"record.lifecycle.runtime_migration_admission": {"authority_id":"scoped"}},
            false,
        ),
        (
            "custom runtime",
            doc! {"record.lifecycle.custom_runtime": {"effective_image":"explicit"}},
            false,
        ),
    ] {
        store
            .sandboxes
            .replace_one(key.clone(), parent.clone())
            .await
            .unwrap();
        if !fields.is_empty() {
            store
                .sandboxes
                .update_one(key.clone(), doc! {"$set":fields})
                .await
                .unwrap();
        }
        for pipeline in [
            discovery::pipeline(&store, &now, 1, &Default::default()),
            discovery::owned_pipeline(&store, &now, 1, None, &Default::default()),
        ] {
            let rows: Vec<Document> = store
                .agents
                .aggregate(pipeline)
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            assert_eq!(rows.is_empty(), held, "{label}");
        }
    }
    client.database(&name).drop().await.unwrap();
}
