use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn home_restore_reader_preserves_nested_shapes_and_refuses_unknown_fields() {
    let (client, name) = database("restore_reader").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let original = sandbox("sbx_restore_reader");
    store.create_sandbox(original.clone()).await.unwrap();
    let raw = client.database(&name).collection::<Document>(SANDBOXES);
    let legacy = raw
        .find_one(doc! {"_id": original.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let lifecycle = legacy
        .get_document("record")
        .unwrap()
        .get_document("lifecycle")
        .unwrap();
    assert!(!lifecycle.contains_key("home_restore"));
    assert!(!lifecycle.contains_key("superseded_homes"));
    assert_eq!(
        store.get_sandbox(&original.id).await.unwrap(),
        Some(original.clone())
    );
    let mut next = original.clone();
    next.lifecycle.home_restore = Some(Box::new(
        serde_json::from_value(serde_json::json!({
            "restore_id": "restore-example", "backup_id": "hb-example-1",
            "operation_id": "sandbox-lifecycle-v1:sbx_restore_reader:2", "requested_at": NOW,
            "requested_by": "operator-example", "key_sha256": "a".repeat(64),
            "evidence": {"kind": "gone", "observed_at": NOW,
                "observed_storage_generation": "old-generation"},
            "replace_existing_home": {"storage_generation": "old-generation"},
            "receipt": {
                "inventory_sha256": "b".repeat(64), "restored_to": NOW,
                "completed_at": NOW, "home_identity": "restored-home-example",
                "source_generation": 1, "covered_agents": ["agent-covered"],
                "uncovered_agents": ["agent-uncovered"],
                "missing_workspaces": ["workspace-missing"],
                "missing_worktrees": ["worktree-missing"],
                "removed_agent_homes": ["agent-removed"],
                "removed_worktrees": ["worktree-removed"]
            }
        }))
        .unwrap(),
    ));
    let mut old_home = layer2_core::sandbox::SandboxHome::reserve(
        &next.id,
        layer3_adapter::HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture-storage".into(),
            capacity_gib: 64,
            template_name: "fixture-seed".into(),
            template_generation: "seed-generation".into(),
        },
    )
    .unwrap();
    old_home
        .bind(layer3_adapter::HomeVolumeReceipt {
            native_authority: None,
            volume_id: old_home.request.volume_id.clone(),
            request_sha256: old_home.request.computed_sha256(),
            storage_reference: "old-home-example".into(),
            storage_generation: "old-generation".into(),
        })
        .unwrap();
    next.lifecycle.runtime_generation_high_water = 1;
    next.lifecycle.superseded_homes = vec![layer2_core::sandbox::SandboxSupersededHome {
        restore_id: "restore-example".into(),
        home_identity: old_home.attachment().unwrap().identity().unwrap(),
        home: Box::new(old_home),
        source_generation: 1,
        superseded_at: NOW.into(),
    }];
    next.revision += 1;
    store
        .save_sandbox(original.revision, next.clone())
        .await
        .unwrap();
    // A fresh store exercises the actual Mongo serializer/strict reader and a
    // subsequent whole-record replacement must preserve the retained pin.
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_sandbox(&next.id).await.unwrap(),
        Some(next.clone())
    );
    let mut stale = original.clone();
    stale.revision += 1;
    stale.name = "stale-writer".into();
    assert!(matches!(
        reopened.save_sandbox(original.revision, stale).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        reopened.get_sandbox(&next.id).await.unwrap(),
        Some(next.clone())
    );
    let revision = next.revision;
    next.name = "renamed-example".into();
    next.revision += 1;
    reopened.save_sandbox(revision, next.clone()).await.unwrap();
    assert_eq!(
        reopened.get_sandbox(&next.id).await.unwrap(),
        Some(next.clone())
    );
    let kind = SandboxEventKind::HomeRestored {
        restore_id: "restore-example".into(),
        backup_id: "hb-example-1".into(),
        restored_to: NOW.into(),
    };
    reopened
        .append_sandbox_event(&next.id, NOW.into(), kind.clone())
        .await
        .unwrap();
    assert_eq!(
        reopened.sandbox_events(&next.id, 0, 10).await.unwrap()[0].kind,
        kind
    );
    let corrupted = client
        .database(&name)
        .collection::<Document>(SANDBOXES)
        .update_one(
            doc! {"_id": next.id.as_str()},
            doc! {"$set": {"record.lifecycle.home_restore.evidence.future": true}},
        )
        .await
        .unwrap();
    assert_eq!(corrupted.matched_count, 1);
    assert!(matches!(
        reopened.get_sandbox(&next.id).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

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
    store.seed_retained_for_test(&expected).await;
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
async fn default_profiles_quarantine_malformed_payloads_without_replacing_them() {
    use layer2_core::{
        AgentProfileId, WorkspaceProfileId, DEFAULT_AGENT_PROFILE_ID, DEFAULT_PROFILE_REVISION,
        DEFAULT_WORKSPACE_PROFILE_ID,
    };

    let (client, name) = database("default_profile_quarantine").await;
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

    let agent = AgentProfileId::new(DEFAULT_AGENT_PROFILE_ID);
    let workspace = WorkspaceProfileId::new(DEFAULT_WORKSPACE_PROFILE_ID);
    let draft_key = doc! {"_id": agent.as_str()};
    let revision_key =
        doc! {"profile_id": agent.as_str(), "revision": i64::from(DEFAULT_PROFILE_REVISION)};
    for (collection, key) in [
        (&store.agent_profiles, draft_key.clone()),
        (&store.agent_revisions, revision_key.clone()),
    ] {
        collection
            .update_one(key, doc! {"$set": {"record": {"unexpected": true}}})
            .await
            .unwrap();
    }
    let draft = store
        .agent_profiles
        .find_one(draft_key.clone())
        .await
        .unwrap();
    let revision = store
        .agent_revisions
        .find_one(revision_key.clone())
        .await
        .unwrap();
    for _ in 0..2 {
        store
            .seed_default_profiles(&spec, bounds, &[])
            .await
            .unwrap();
        assert_eq!(
            store
                .agent_profiles
                .find_one(draft_key.clone())
                .await
                .unwrap(),
            draft
        );
        assert_eq!(
            store
                .agent_revisions
                .find_one(revision_key.clone())
                .await
                .unwrap(),
            revision
        );
    }
    assert!(matches!(
        store.agent_profile(&agent).await,
        Err(ProfileStoreError::Corrupt(_))
    ));
    assert!(matches!(
        store
            .agent_profile_revision(&agent, DEFAULT_PROFILE_REVISION)
            .await,
        Err(ProfileStoreError::Corrupt(_))
    ));
    assert!(store.workspace_profile(&workspace).await.unwrap().is_some());
    assert!(store
        .workspace_profile_revision(&workspace, DEFAULT_PROFILE_REVISION)
        .await
        .unwrap()
        .is_some());

    store
        .agent_revisions
        .update_one(
            revision_key,
            doc! {"$set": {"document_schema_version": 999_i64}},
        )
        .await
        .unwrap();
    assert!(matches!(
        store.seed_default_profiles(&spec, bounds, &[]).await,
        Err(layer2_core::StoreError::Malformed(_))
    ));
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
