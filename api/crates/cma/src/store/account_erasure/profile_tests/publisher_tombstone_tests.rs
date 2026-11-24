use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_tombstone_intermediate_states_are_unavailable_without_hiding_corruption() {
    let seed = seeded("erasure_trigger_states").await;
    let db = seed.client.database(&seed.database);
    let id = TriggerId("trg_erasure_intermediate".into());
    seed.store
        .create_trigger(trigger(id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    let public = seed
        .store
        .publish_trigger(&id, &seed.owner, 1, NOW.into())
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_some());
    assert!(seed
        .store
        .preview_by_public_id(&public.public_id, "Publisher".into())
        .await
        .unwrap()
        .is_some());
    let definitions = db.collection::<Document>("trigger_definitions");
    let definition = definitions
        .find_one(doc! {"_id":id.as_str()})
        .await
        .unwrap()
        .unwrap();
    definitions
        .delete_one(doc! {"_id":id.as_str()})
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_none());
    assert!(seed
        .store
        .preview_by_public_id(&public.public_id, "Publisher".into())
        .await
        .unwrap()
        .is_none());
    assert!(
        !seed
            .store
            .trigger_revision(&id, &public.revision.digest)
            .await
            .unwrap()
            .unwrap()
            .publisher_deleted
    );
    definitions.insert_one(definition).await.unwrap();
    let revisions = db.collection::<Document>("trigger_definition_revisions");
    let row = revisions
        .find_one(doc! {"trigger_id":id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let subjects = std::collections::BTreeSet::from([seed.owner.to_string()]);
    super::references::redact(&db, "trigger_definition_revisions", &row, &subjects)
        .await
        .unwrap();
    let canonical = revisions
        .find_one(doc! {"trigger_id":id.as_str()})
        .await
        .unwrap()
        .unwrap();
    revisions.update_one(doc! {"trigger_id":id.as_str()}, doc! {"$set":{"published_at":"2026-11-15T00:00:00Z","record.published_at":"2026-11-15T00:00:00Z"}}).await.unwrap();
    assert!(
        matches!(
            seed.store.trigger_by_public_id(&public.public_id).await,
            Err(layer2_core::trigger_domain::TriggerStoreError::Corrupt(_))
        ),
        "a canonical marker cannot authorize changed publication metadata"
    );
    revisions
        .replace_one(doc! {"trigger_id":id.as_str()}, canonical)
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_none());
    assert!(seed
        .store
        .preview_by_public_id(&public.public_id, "Publisher".into())
        .await
        .unwrap()
        .is_none());
    db.collection::<Document>("trigger_definitions")
        .delete_one(doc! {"_id":id.as_str()})
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_none());
    assert!(seed
        .store
        .preview_by_public_id(&public.public_id, "Publisher".into())
        .await
        .unwrap()
        .is_none());
    assert!(
        seed.store
            .trigger_revision(&id, &public.revision.digest)
            .await
            .unwrap()
            .unwrap()
            .publisher_deleted
    );
    let corrupt_id = TriggerId("trg_erasure_corrupt".into());
    seed.store
        .create_trigger(trigger(corrupt_id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    let corrupt_public = seed
        .store
        .publish_trigger(&corrupt_id, &seed.owner, 1, NOW.into())
        .await
        .unwrap();
    revisions
        .update_one(
            doc! {"trigger_id":corrupt_id.as_str()},
            doc! {"$set":{"record.digest":"invalid"}},
        )
        .await
        .unwrap();
    assert!(matches!(
        seed.store
            .trigger_by_public_id(&corrupt_public.public_id)
            .await,
        Err(layer2_core::trigger_domain::TriggerStoreError::Corrupt(_))
    ));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn publisher_tombstone_does_not_hide_later_revision_metadata_corruption() {
    let seed = seeded("erasure_strict_revision_inventory").await;
    let db = seed.client.database(&seed.database);
    let id = TriggerId("trg_erasure_multiple_revisions".into());
    seed.store
        .create_trigger(trigger(id.as_str(), &seed.owner, &seed.target))
        .await
        .unwrap();
    seed.store
        .publish_trigger(&id, &seed.owner, 1, NOW.into())
        .await
        .unwrap();
    let definition = seed.store.trigger(&id, &seed.owner).await.unwrap().unwrap();
    let mut draft = definition.draft;
    draft.presentation.title = "Second revision".into();
    let saved = seed
        .store
        .save_draft(
            &id,
            &seed.owner,
            definition.draft_version,
            draft,
            "2026-11-14T00:01:00Z".into(),
        )
        .await
        .unwrap();
    let public = seed
        .store
        .publish_trigger(
            &id,
            &seed.owner,
            saved.draft_version,
            "2026-11-14T00:01:00Z".into(),
        )
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_some());
    let revisions = db.collection::<Document>("trigger_definition_revisions");
    let first = revisions
        .find_one(doc! {"trigger_id":id.as_str(),"revision":1_i64})
        .await
        .unwrap()
        .unwrap();
    super::references::redact(
        &db,
        "trigger_definition_revisions",
        &first,
        &std::collections::BTreeSet::from([seed.owner.to_string()]),
    )
    .await
    .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_none());
    let second = revisions
        .find_one(doc! {"trigger_id":id.as_str(),"revision":2_i64})
        .await
        .unwrap()
        .unwrap();
    revisions
        .update_one(
            doc! {"trigger_id":id.as_str(),"revision":2_i64},
            doc! {"$set":{
                "published_at":"2026-11-15T00:00:00Z","record.published_at":"2026-11-15T00:00:00Z",
            }},
        )
        .await
        .unwrap();
    assert!(matches!(
        seed.store.trigger_by_public_id(&public.public_id).await,
        Err(layer2_core::trigger_domain::TriggerStoreError::Corrupt(_))
    ));
    assert!(matches!(
        seed.store
            .preview_by_public_id(&public.public_id, "Publisher".into())
            .await,
        Err(layer2_core::trigger_domain::TriggerStoreError::Corrupt(_))
    ));
    revisions
        .replace_one(doc! {"trigger_id":id.as_str(),"revision":2_i64}, second)
        .await
        .unwrap();
    assert!(seed
        .store
        .trigger_by_public_id(&public.public_id)
        .await
        .unwrap()
        .is_none());
    db.drop().await.unwrap();
}
