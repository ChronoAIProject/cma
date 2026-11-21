use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_resource_activity_survives_while_personal_activity_is_deleted() {
    use layer2_core::{
        ActivityAction, ActivityOutcome, ActivityQuery, ActivityRecord, ActivityStore,
        ActivityTarget, DetailKey,
    };
    let db = database().await;
    sandbox(&db, "organization", "sbx_org").await;
    let mut ownership = policy();
    ownership
        .organization_evidence
        .as_mut()
        .unwrap()
        .admitted_by = actor("survivor");
    put(
        &db,
        "resource_policies",
        records::policy_document(&ownership).unwrap(),
    )
    .await;
    let store = crate::store::activity::MongoActivityStore::open(
        db.client(),
        db.name(),
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    let mut originals = vec![];
    for target in ["sandbox:sbx_org", "sandbox:sbx_org:events"] {
        let mut record = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
            .by(SubjectId::new("erased"))
            .on(ActivityTarget::Resource(target.into()));
        record.detail.record(DetailKey::Subject, "organization");
        store.append(&record).await.unwrap();
        originals.push(record);
    }
    let personal = ActivityRecord::new(ActivityAction::Delete, ActivityOutcome::Succeeded)
        .by(SubjectId::new("erased"))
        .on(ActivityTarget::Subject(SubjectId::new("erased")));
    store.append(&personal).await.unwrap();
    let generic = ActivityRecord::new(ActivityAction::Update, ActivityOutcome::Succeeded)
        .by(SubjectId::new("erased"))
        .on(ActivityTarget::Resource(
            "/v1/sandboxes/{id}/runtime-update".into(),
        ));
    store.append(&generic).await.unwrap();
    sandbox(&db, "foreign-person", "sbx_foreign").await;
    let foreign = ActivityRecord::new(ActivityAction::Read, ActivityOutcome::Succeeded)
        .by(SubjectId::new("erased"))
        .on(ActivityTarget::Resource("sandbox:sbx_foreign".into()));
    store.append(&foreign).await.unwrap();
    drain(&db, "erased", &mut ErasureInventory::default()).await;
    let retained = store.list(&ActivityQuery::default()).await.unwrap();
    assert_eq!(retained.items.len(), 2);
    for original in originals {
        let cleaned = retained
            .items
            .iter()
            .find(|record| record.id == original.id)
            .unwrap();
        assert_eq!(cleaned.actor, Some(SubjectId::new("deleted-account")));
        assert_eq!(cleaned.target, original.target);
        assert_eq!(cleaned.detail, original.detail);
        assert_eq!(cleaned.action, original.action);
    }
    assert_eq!(count(&db, "sandboxes").await, 2);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_resource_activity_compares_exact_evidence_before_redaction() {
    use layer2_core::{
        ActivityAction, ActivityOutcome, ActivityRecord, ActivityStore, ActivityTarget,
    };
    let db = database().await;
    sandbox(&db, "organization", "sbx_org").await;
    let mut ownership = policy();
    ownership
        .organization_evidence
        .as_mut()
        .unwrap()
        .admitted_by = actor("survivor");
    put(
        &db,
        "resource_policies",
        records::policy_document(&ownership).unwrap(),
    )
    .await;
    let store = crate::store::activity::MongoActivityStore::open(
        db.client(),
        db.name(),
        std::time::Duration::from_secs(86400),
    )
    .await
    .unwrap();
    let record = ActivityRecord::new(ActivityAction::Update, ActivityOutcome::Succeeded)
        .by(SubjectId::new("erased"))
        .on(ActivityTarget::Resource("sandbox:sbx_org".into()));
    store.append(&record).await.unwrap();
    let stale = row(&db, "activity", &record.id.0).await;
    db.collection::<Document>("activity")
        .update_one(
            doc! {"_id":&record.id.0},
            doc! {"$set":{"record.detail.status":"concurrent-status"}},
        )
        .await
        .unwrap();
    assert_eq!(
        redact(&db, "activity", &stale, &subjects()).await.unwrap(),
        Redaction::Conflict
    );
    let current = row(&db, "activity", &record.id.0).await;
    assert_eq!(
        redact(&db, "activity", &current, &subjects())
            .await
            .unwrap(),
        Redaction::Updated
    );
    let cleaned = row(&db, "activity", &record.id.0).await;
    assert_eq!(
        super::selectors::strings(&cleaned, "record.actor"),
        ["deleted-account"]
    );
    assert_eq!(
        super::selectors::strings(&cleaned, "record.detail.status"),
        ["concurrent-status"]
    );
    db.drop().await.unwrap();
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
