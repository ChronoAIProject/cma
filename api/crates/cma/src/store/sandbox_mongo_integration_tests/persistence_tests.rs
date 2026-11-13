use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn encrypted_credential_round_trip_hides_plaintext_and_rejects_tampering() {
    use mongodb::bson::{spec::BinarySubtype, Binary, Bson};

    let (client, name) = database("credentials").await;
    let store =
        MongoSandboxCredentialStore::open(&client, &name, "test-deployment", &encryption_key())
            .await
            .unwrap();
    let expected = credential("sbx_credential", "credential-plaintext");
    store.insert(expected.clone()).await.unwrap();
    let raw = client
        .database(&name)
        .collection::<Document>(SANDBOX_CREDENTIALS);
    let document = raw.find_one(Document::new()).await.unwrap().unwrap();
    assert!(!format!("{document:?}").contains("credential-plaintext"));
    let loaded = store
        .get(&expected.sandbox_id, expected.authority.generation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.authority, expected.authority);
    assert_eq!(loaded.owner, expected.owner);
    assert_eq!(loaded.value.expose(), "credential-plaintext");

    let revocation = CredentialRevocation {
        sandbox_id: expected.sandbox_id.clone(),
        owner: expected.owner.clone(),
        provider_key_id: "provider-key-loser".into(),
    };
    store.queue_revocation(revocation.clone()).await.unwrap();
    store.queue_revocation(revocation.clone()).await.unwrap();
    assert_eq!(
        store.revocations(&expected.sandbox_id).await.unwrap(),
        vec![revocation.clone()]
    );
    store
        .delete_revocation(&expected.sandbox_id, &revocation.provider_key_id)
        .await
        .unwrap();
    assert!(store
        .revocations(&expected.sandbox_id)
        .await
        .unwrap()
        .is_empty());

    raw.update_one(
        doc! { "_id": document.get_str("_id").unwrap() },
        doc! { "$set": {
            "value.ciphertext": Bson::Binary(Binary {
                subtype: BinarySubtype::Generic,
                bytes: vec![1, 2, 3],
            })
        } },
    )
    .await
    .unwrap();
    assert!(matches!(
        store
            .get(&expected.sandbox_id, expected.authority.generation)
            .await,
        Err(CredentialCustodyError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn default_profiles_seed_idempotently_and_refuse_configuration_drift() {
    let (client, name) = database("default_profiles").await;
    let store = MongoProfileStore::open(&client, &name).await.unwrap();
    let bounds = layer2_core::profiles::ProfileCapacityBounds {
        min_cpu_millis: 100,
        max_cpu_millis: 8_000,
        min_memory_mib: 128,
        max_memory_mib: 16_384,
    };
    let spec = DefaultProfileSpec {
        cpu_millis: 1_000,
        memory_mib: 1_024,
        agent_model: "model-a".into(),
        agent_effort: ThinkingEffort::High,
    };
    store
        .seed_default_profiles(&spec, bounds, &[])
        .await
        .unwrap();
    store
        .seed_default_profiles(&spec, bounds, &[])
        .await
        .unwrap();

    for collection in [
        &store.agent_profiles,
        &store.workspace_profiles,
        &store.sandbox_profiles,
    ] {
        collection
            .update_many(
                doc! {},
                doc! {"$set": {"uses_count": 7_i64, "record.uses_count": 7_i64}},
            )
            .await
            .unwrap();
    }
    store
        .seed_default_profiles(&spec, bounds, &[])
        .await
        .unwrap();
    for collection in [
        &store.agent_profiles,
        &store.workspace_profiles,
        &store.sandbox_profiles,
    ] {
        assert_eq!(
            collection
                .find_one(doc! {})
                .await
                .unwrap()
                .unwrap()
                .get_i64("uses_count")
                .unwrap(),
            7
        );
    }

    let mut changed = spec;
    changed.agent_model = "model-b".into();
    assert!(matches!(
        store.seed_default_profiles(&changed, bounds, &[]).await,
        Err(layer2_core::StoreError::Malformed(_))
    ));
    assert_eq!(
        store
            .agent_revisions
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn startup_refuses_an_unknown_document_schema_version() {
    let (client, name) = database("unknown_version").await;
    client
        .database(&name)
        .collection::<Document>(SANDBOXES)
        .insert_one(doc! { "_id": "future", "document_schema_version": 2_i64 })
        .await
        .unwrap();
    let error = match MongoSandboxStore::open(&client, &name).await {
        Ok(_) => panic!("an unknown schema version must stop startup"),
        Err(error) => error,
    };
    assert!(matches!(error, layer2_core::StoreError::Malformed(_)));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn unique_names_and_revision_cas_return_typed_errors() {
    let (client, name) = database("cas").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let original = sandbox("sbx_cas_one");
    store.create_sandbox(original.clone()).await.unwrap();
    let mut same_id = original.clone();
    same_id.name = "must-not-replace".into();
    assert_eq!(
        store.create_sandbox(same_id).await,
        Err(SandboxStoreError::Duplicate)
    );
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(original.clone())
    );
    assert_eq!(
        store.save_sandbox(u64::MAX, original.clone()).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(original.clone())
    );
    let mut duplicate = original.clone();
    duplicate.id = SandboxId("sbx_cas_two".into());
    assert_eq!(
        store.create_sandbox(duplicate).await,
        Err(SandboxStoreError::Duplicate)
    );
    let mut replacement = original.clone();
    replacement.revision = 2;
    replacement.updated_at = "2026-09-05T00:00:01Z".into();
    store.save_sandbox(1, replacement.clone()).await.unwrap();
    replacement.updated_at = "2026-09-05T00:00:02Z".into();
    assert_eq!(
        store.save_sandbox(1, replacement).await,
        Err(SandboxStoreError::Conflict)
    );
    client.database(&name).drop().await.unwrap();
}
