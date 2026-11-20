//! Foreign activity targets cannot create personal ownership.
use super::{tests::*, *};
use layer2_core::SandboxId;
use mongodb::bson::{doc, Document};

async fn row(db: &Database, name: &str, id: &str) -> Document {
    db.collection::<Document>(name)
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn operator_activity_never_claims_foreign_orphans_without_ownership_evidence() {
    use layer2_core::sandbox::{SandboxEvent, SandboxEventKind};
    use layer2_core::{
        ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore, ActivityTarget,
    };
    let db = database().await;
    let store = crate::store::activity::MongoActivityStore::open(
        db.client(),
        db.name(),
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    let activity = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
        .by(SubjectId::new("erased"))
        .on(ActivityTarget::Resource(
            "sandbox:sbx_foreign_removed:events".into(),
        ));
    store.append(&activity).await.unwrap();
    let event = SandboxEvent {
        sandbox_id: SandboxId("sbx_foreign_removed".into()),
        seq: 1,
        appended_at: NOW.into(),
        kind: SandboxEventKind::Created,
    };
    let original = crate::store::sandbox_events::event_document(&event).unwrap();
    let id = original.get_str("_id").unwrap().to_owned();
    put(&db, "sandbox_events", original.clone()).await;
    let mut inventory = ErasureInventory::default();
    drain(&db, "erased", &mut inventory).await;
    assert_eq!(count(&db, "activity").await, 0);
    assert_eq!(row(&db, "sandbox_events", &id).await, original);
    assert_eq!(
        crate::store::sandbox_events::read_event(&row(&db, "sandbox_events", &id).await).unwrap(),
        event
    );
    assert!(inventory.sandboxes.is_empty());
    assert_eq!(count(&db, "sandboxes").await, 0);
    assert_eq!(count(&db, "resource_policies").await, 0);
    db.drop().await.unwrap();
}
