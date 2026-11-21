use layer2_core::ids::SubjectId;
use layer2_core::trigger_domain::{
    TriggerAvailability, TriggerId, TriggerPageRequest, TriggerStore, TriggerStoreError,
};
use mongodb::bson::{doc, Bson};

use super::documents::{read_trigger, trigger_document};
use super::integration_tests::{seeded, trigger};

const NOW: &str = "2026-10-19T02:00:00Z";

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn archive_hides_owner_records_retains_revisions_and_rejects_mutations() {
    let seed = seeded("archive").await;
    for (id, published, retired) in [
        ("trg_archive_draft", false, false),
        ("trg_archive_published", true, false),
        ("trg_archive_retired", true, true),
    ] {
        let definition = trigger(id, &seed.owner, &seed.target);
        let original_draft = definition.draft.clone();
        let id = definition.id.clone();
        seed.store.create_trigger(definition).await.unwrap();
        let public = if published {
            Some(
                seed.store
                    .publish_trigger(&id, &seed.owner, 1, NOW.into())
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        if published {
            seed.store.record_launch(&id).await.unwrap();
        }
        if retired {
            seed.store
                .retire_trigger(&id, &seed.owner, NOW.into())
                .await
                .unwrap();
        }
        assert_eq!(
            seed.store
                .delete_trigger(&id, &SubjectId::new("other"), NOW.into())
                .await,
            Err(TriggerStoreError::NotFound)
        );
        seed.store
            .delete_trigger(&id, &seed.owner, NOW.into())
            .await
            .unwrap();
        assert!(seed
            .store
            .trigger(&id, &seed.owner)
            .await
            .unwrap()
            .is_none());
        assert!(seed
            .store
            .list_own_triggers(
                &seed.owner,
                &TriggerPageRequest {
                    limit: 1,
                    cursor: None
                }
            )
            .await
            .unwrap()
            .items
            .is_empty());
        assert_eq!(
            seed.store
                .publish_trigger(&id, &seed.owner, 1, NOW.into())
                .await,
            Err(TriggerStoreError::NotFound)
        );
        assert_eq!(
            seed.store
                .save_draft(&id, &seed.owner, 1, original_draft, NOW.into())
                .await,
            Err(TriggerStoreError::NotFound)
        );
        assert_eq!(
            seed.store
                .create_placement(
                    &id,
                    &seed.owner,
                    "https://example.com/archived".into(),
                    "archived".into()
                )
                .await,
            Err(TriggerStoreError::NotFound)
        );
        let stored = seed
            .store
            .definitions
            .find_one(doc! { "_id": id.as_str() })
            .await
            .unwrap();
        if let Some(public) = public {
            assert!(seed.store.record_launch(&id).await.is_err());
            assert_eq!(
                seed.store
                    .record_agent_ready(&id)
                    .await
                    .unwrap()
                    .agents_ready_total,
                1
            );
            let document = stored.unwrap();
            let archived = read_trigger(&document).unwrap().trigger;
            assert_eq!(archived.archived_at.as_deref(), Some(NOW));
            assert_eq!(document.get_str("archived_at").unwrap(), NOW);
            let retained = seed
                .store
                .trigger_by_public_id(&public.public_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(retained.availability, TriggerAvailability::Retired);
            assert_eq!(retained.revision, public.revision);
            assert_eq!(
                seed.store
                    .trigger_revision(&id, &public.revision.digest)
                    .await
                    .unwrap(),
                Some(public.revision)
            );
            assert_eq!(
                seed.store
                    .revisions
                    .count_documents(doc! { "trigger_id": id.as_str() })
                    .await
                    .unwrap(),
                1
            );
        } else {
            assert!(stored.is_none());
        }
    }
    let visible = trigger("trg_archive_visible", &seed.owner, &seed.target);
    seed.store.create_trigger(visible.clone()).await.unwrap();
    let page = seed
        .store
        .list_own_triggers(
            &seed.owner,
            &TriggerPageRequest {
                limit: 1,
                cursor: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(page.items, vec![visible]);
    assert!(page.next.is_none());
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn archive_and_publication_serialize_without_orphaning_revisions() {
    let seed = seeded("archive_race").await;
    for n in 0..8 {
        let id = TriggerId(format!("trg_archive_race_{n}"));
        seed.store
            .create_trigger(trigger(id.as_str(), &seed.owner, &seed.target))
            .await
            .unwrap();
        let (published, deleted) = tokio::join!(
            seed.store.publish_trigger(&id, &seed.owner, 1, NOW.into()),
            seed.store.delete_trigger(&id, &seed.owner, NOW.into()),
        );
        deleted.unwrap();
        assert!(seed
            .store
            .trigger(&id, &seed.owner)
            .await
            .unwrap()
            .is_none());
        if let Ok(public) = published {
            let retained = seed
                .store
                .trigger_by_public_id(&public.public_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(retained.availability, TriggerAvailability::Retired);
            assert_eq!(retained.revision, public.revision);
        } else {
            assert!(seed
                .store
                .definitions
                .find_one(doc! { "_id": id.as_str() })
                .await
                .unwrap()
                .is_none());
            assert_eq!(
                seed.store
                    .revisions
                    .count_documents(doc! { "trigger_id": id.as_str() })
                    .await
                    .unwrap(),
                0
            );
        }
    }
    seed.client.database(&seed.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn legacy_records_remain_visible_and_archive_projection_disagreement_is_refused() {
    let seed = seeded("archive_legacy").await;
    let definition = trigger("trg_archive_legacy", &seed.owner, &seed.target);
    let mut document = trigger_document(&definition, 1).unwrap();
    document.remove("archived_at");
    assert_eq!(read_trigger(&document).unwrap().trigger, definition);
    seed.store
        .definitions
        .insert_one(document.clone())
        .await
        .unwrap();
    assert_eq!(
        seed.store
            .trigger(&definition.id, &seed.owner)
            .await
            .unwrap(),
        Some(definition.clone())
    );
    document.insert("archived_at", NOW);
    assert!(read_trigger(&document).is_err());
    seed.store
        .publish_trigger(&definition.id, &seed.owner, 1, NOW.into())
        .await
        .unwrap();
    seed.store
        .delete_trigger(&definition.id, &seed.owner, NOW.into())
        .await
        .unwrap();
    let mut archived = seed
        .store
        .definitions
        .find_one(doc! { "_id": definition.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    archived.insert("archived_at", Bson::Null);
    assert!(read_trigger(&archived).is_err());
    seed.client.database(&seed.database).drop().await.unwrap();
}
