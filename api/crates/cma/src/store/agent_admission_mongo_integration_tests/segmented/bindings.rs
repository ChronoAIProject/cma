use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn segmented_reuse_binding_requires_its_versioned_publication() {
    let (client, database, store, request, _) = adopted("reuse_binding_shape").await;
    store
        .admit_agent_reusing_profile(request.clone())
        .await
        .unwrap();
    let id = crate::store::agent_admission_records::binding_id(&request).unwrap();
    let original = store
        .agent_reuse_bindings
        .find_one(doc! {"_id":&id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.get_i64("document_schema_version").unwrap(), 2);
    for update in [
        doc! {"$unset":{"log_publication":""}},
        doc! {"$set":{"document_schema_version":3_i64}},
        doc! {"$set":{"document_schema_version":1_i64}},
    ] {
        store
            .agent_reuse_bindings
            .update_one(doc! {"_id":&id}, update)
            .await
            .unwrap();
        assert!(store
            .admit_agent_reusing_profile(request.clone())
            .await
            .is_err());
        store
            .agent_reuse_bindings
            .replace_one(doc! {"_id":&id}, original.clone())
            .await
            .unwrap();
    }
    store.admit_agent_reusing_profile(request).await.unwrap();
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn legacy_reuse_binding_remains_replayable_after_adoption() {
    let (client, database, store, _, _, mut original) = fixture("legacy_reuse_then_adopt").await;
    let creation = store
        .admit_agent(original.clone())
        .await
        .unwrap()
        .creation()
        .clone();
    original.creation_key = "later-legacy-trigger".into();
    original.agent_id = AgentId::generate();
    original.worktree_id = WorktreeId::generate();
    original.origin = AgentOrigin::Trigger {
        trigger_id: "trg_legacy".into(),
        launch_id: "lch_legacy".into(),
    };
    original.first_message = Some(
        layer2_core::sandbox::FirstMessageRecord::new(
            original.creation_key.clone(),
            "Original prompt",
            NOW,
        )
        .unwrap(),
    );
    store
        .admit_agent_reusing_profile(original.clone())
        .await
        .unwrap();
    let binding = store
        .agent_reuse_bindings
        .find_one(doc! {})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(binding.get_i64("document_schema_version").unwrap(), 1);
    assert!(!binding.contains_key("log_publication"));
    schema::migrate(&client.database(&database)).await.unwrap();
    crate::store::migrations::agent_log_source_keys::prepare_for_test(&client.database(&database))
        .await;
    gate(&store, &creation.agent.id, 0, true).await;
    append(&store, &creation.agent.id, "adopt-after-legacy-reuse").await;
    gate(&store, &creation.agent.id, 1, false).await;
    original.agent_id = AgentId::generate();
    original.worktree_id = WorktreeId::generate();
    original.event_at = "2026-09-05T00:01:00Z".into();
    assert_eq!(
        store
            .admit_agent_reusing_profile(original)
            .await
            .unwrap()
            .creation()
            .agent
            .id,
        creation.agent.id
    );
    client.database(&database).drop().await.unwrap();
}
