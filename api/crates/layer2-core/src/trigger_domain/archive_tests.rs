use super::tests::{seed_profiles, trigger, trigger_store, Visibilities};
use super::*;
use crate::SubjectId;

const PUBLISHED: &str = "2026-10-19T01:00:00Z";
const ARCHIVED: &str = "2026-10-19T02:00:00Z";

#[tokio::test]
async fn delete_removes_drafts_but_retains_published_authority_and_hides_archived_definitions() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let store = trigger_store(&seed);
    for published in [false, true] {
        let definition = trigger(seed.owner.clone(), seed.target.clone());
        let id = definition.id.clone();
        store.create_trigger(definition).await.unwrap();
        let public = if published {
            Some(
                store
                    .publish_trigger(&id, &seed.owner, 1, PUBLISHED.into())
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        if published {
            store.record_launch(&id).await.unwrap();
        }
        assert_eq!(
            store
                .delete_trigger(&id, &SubjectId::new("other"), ARCHIVED.into())
                .await,
            Err(TriggerStoreError::NotFound)
        );
        store
            .delete_trigger(&id, &seed.owner, ARCHIVED.into())
            .await
            .unwrap();
        assert!(store.trigger(&id, &seed.owner).await.unwrap().is_none());
        assert!(store
            .list_own_triggers(&seed.owner, &TriggerPageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
        assert_eq!(
            store
                .publish_trigger(&id, &seed.owner, 1, ARCHIVED.into())
                .await,
            Err(TriggerStoreError::NotFound)
        );
        assert_eq!(
            store
                .save_draft(
                    &id,
                    &seed.owner,
                    1,
                    trigger(seed.owner.clone(), seed.target.clone()).draft,
                    ARCHIVED.into()
                )
                .await,
            Err(TriggerStoreError::NotFound)
        );
        assert_eq!(
            store
                .create_placement(
                    &id,
                    &seed.owner,
                    "https://example.com/archived".into(),
                    "archived".into()
                )
                .await,
            Err(TriggerStoreError::NotFound)
        );
        if let Some(public) = public {
            assert!(store.record_launch(&id).await.is_err());
            assert_eq!(
                store
                    .record_agent_ready(&id)
                    .await
                    .unwrap()
                    .agents_ready_total,
                1
            );
            let retained = store
                .trigger_by_public_id(&public.public_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(retained.availability, TriggerAvailability::Retired);
            assert_eq!(retained.revision, public.revision);
            assert_eq!(
                store
                    .trigger_revision(&id, &public.revision.digest)
                    .await
                    .unwrap(),
                Some(public.revision)
            );
        } else {
            // The same ID can be created again only because an unpublished draft was removed.
            let mut replacement = trigger(seed.owner.clone(), seed.target.clone());
            replacement.id = id.clone();
            store.create_trigger(replacement).await.unwrap();
            store
                .delete_trigger(&id, &seed.owner, ARCHIVED.into())
                .await
                .unwrap();
        }
    }
}

#[tokio::test]
async fn archived_aggregate_preserves_history_and_rejects_mutation() {
    let seed = seed_profiles(Visibilities::default(), true).await;
    let mut definition = trigger(seed.owner, seed.target);
    let old_json = serde_json::to_value(&definition).unwrap();
    assert!(old_json.get("archived_at").is_none());
    assert_eq!(
        serde_json::from_value::<Trigger>(old_json)
            .unwrap()
            .archived_at,
        None
    );
    definition.publish(PUBLISHED.into()).unwrap();
    let revision = definition.revisions.clone();
    definition.record_launch().unwrap();
    definition.retire(PUBLISHED.into()).unwrap();
    definition.archive(ARCHIVED.into()).unwrap();
    definition.archive("2026-10-19T03:00:00Z".into()).unwrap();
    assert_eq!(definition.archived_at.as_deref(), Some(ARCHIVED));
    assert_eq!(definition.retired_at.as_deref(), Some(PUBLISHED));
    assert_eq!(definition.revisions, revision);
    assert!(definition.publish(ARCHIVED.into()).is_err());
    assert!(definition.record_launch().is_err());
    definition.record_agent_ready().unwrap();
    definition.validate().unwrap();
    definition.availability = TriggerAvailability::Published;
    assert!(definition.validate().is_err());
}
