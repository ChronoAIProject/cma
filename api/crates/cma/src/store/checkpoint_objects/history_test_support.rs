//! Narrow test support for exercising the actual private tombstone transaction.
use super::{
    model::{ArchiveIdentity, ArchiveState, ManifestDoc},
    MongoCheckpointObjectStore,
};
use layer2_core::bridge::allocation::AllocationRecord;
use mongodb::{
    bson::{doc, DateTime, Document},
    Client, Database,
};

pub(in crate::store) async fn check(
    objects: &MongoCheckpointObjectStore,
    client: &Client,
    db: &Database,
    allocation: &AllocationRecord,
) {
    let bytes = b"uncertain original upload";
    let sha256 = "a".repeat(64);
    let reference = format!("home-{sha256}.cma");
    let identity = ArchiveIdentity::from_input(reference, sha256, bytes.len() as u64).unwrap();
    let mut stage = ManifestDoc::new(
        allocation.placement.target().sandbox_id.as_str(),
        &identity,
        DateTime::now(),
    );
    stage.state = ArchiveState::Deleting;
    objects
        .manifests
        .insert_one(mongodb::bson::serialize_to_document(&stage).unwrap())
        .await
        .unwrap();
    db.run_command(doc! {"collMod":"channel_history_references", "validator":{"denied":{"$exists":true}}, "validationLevel":"strict", "validationAction":"error"}).await.unwrap();
    assert!(super::tombstones::retain(objects, &stage, DateTime::now())
        .await
        .is_err());
    assert!(objects
        .manifests
        .find_one(doc! {"_id": &stage.id})
        .await
        .unwrap()
        .is_some());
    assert!(objects
        .tombstones
        .find_one(doc! {"_id": &stage.id})
        .await
        .unwrap()
        .is_none());
    db.run_command(doc! {"collMod":"channel_history_references", "validator":{}})
        .await
        .unwrap();
    let now = DateTime::now();
    assert!(super::tombstones::retain(objects, &stage, now)
        .await
        .unwrap());
    let references = db.collection::<Document>("channel_history_references");
    let reference = references
        .find_one(doc! {"allocation_id": &allocation.id, "record.kind":"late_upload_tombstone"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reference
            .get_document("record")
            .unwrap()
            .get_str("target_id")
            .unwrap(),
        stage.id
    );
    super::tombstones::cleanup_due(
        objects,
        DateTime::from_millis(now.timestamp_millis() + 61_000),
    )
    .await
    .unwrap();
    assert!(objects
        .tombstones
        .find_one(doc! {"_id": &stage.id})
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        references
            .find_one(doc! {"_id": reference.get_str("_id").unwrap()})
            .await
            .unwrap()
            .unwrap(),
        reference
    );
    let mut session = client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let document = db
        .collection::<Document>("bridge_allocations")
        .find_one(doc! {"_id": &allocation.id})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    let assessment = crate::store::channel_history::assess(db, allocation, &document, &mut session)
        .await
        .unwrap();
    assert_eq!(assessment.hold, None);
    assert_eq!(assessment.reference_count, 2);
    assert!(!assessment.inventory_sealed);
    session.abort_transaction().await.unwrap();
}
