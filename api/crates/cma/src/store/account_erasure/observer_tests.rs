use super::{tests::*, *};
use crate::store::agent_observation::COLLECTION;
use mongodb::bson::{doc, Bson, Document};

fn observer(id: &str) -> Document {
    doc! {"_id":id,"document_schema_version":1_i64,"created_at":NOW,
    "instance_id":"retained-observer","epoch":7_i64,
    "expires_at":mongodb::bson::DateTime::from_millis(0),
    "next_refresh_at":mongodb::bson::DateTime::from_millis(0),"round":""}
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn observers_follow_acknowledged_agents_and_preserve_foreign_and_organization_rows() {
    let db = database().await;
    for (id, owner) in [
        ("sbx_a", "acct:a"),
        ("sbx_b", "acct:b"),
        ("sbx_org", "acct:a"),
    ] {
        sandbox(&db, owner, id).await;
    }
    put(
        &db,
        "resource_policies",
        doc! {"_id":"sbx_org","owner_kind":"organization","owner_subject":"org:retained"},
    )
    .await;
    for (agent, sandbox, owner) in [
        ("agt_a", "sbx_a", "acct:a"),
        ("agt_b", "sbx_b", "acct:b"),
        ("agt_org", "sbx_org", "acct:a"),
    ] {
        put(
            &db,
            "agents",
            doc! {"_id":agent,"sandbox_id":sandbox,"owner":owner},
        )
        .await;
        put(&db, COLLECTION, observer(agent)).await;
    }
    for (id, kind, subject) in [
        ("orphan_a", "person", "acct:a"),
        ("orphan_org", "organization", "org:retained"),
    ] {
        put(
            &db,
            "agent_creation_provenance",
            doc! {"_id":id,"record":{"owner":{"kind":kind,"subject":subject},"agent_id":id}},
        )
        .await;
        put(&db, COLLECTION, observer(id)).await;
    }
    let mut inventory = ErasureInventory {
        // Historical input deliberately includes foreign and organization IDs.
        agents: vec!["agt_b".into(), "agt_org".into(), "orphan_org".into()],
        ..Default::default()
    };
    let first = erase_records(
        &db,
        &[Target {
            subject: &SubjectId::new("acct:a"),
            inventory: &inventory,
        }],
        &mut Cursor::default(),
        NOW,
    )
    .await
    .unwrap();
    assert!(first.waiting);
    assert_eq!(count(&db, COLLECTION).await, 5);
    let deleted = drain(&db, "acct:a", &mut inventory).await;
    assert_eq!(deleted.get(COLLECTION), Some(&2));
    assert_eq!(count(&db, COLLECTION).await, 3);
    for id in ["agt_b", "agt_org", "orphan_org"] {
        assert_eq!(
            db.collection::<Document>(COLLECTION)
                .find_one(doc! {"_id":id})
                .await
                .unwrap(),
            Some(observer(id))
        );
    }
    assert!(db
        .collection::<Document>("agents")
        .find_one(doc! {"_id":"agt_a"})
        .await
        .unwrap()
        .is_none());
    assert_eq!(count(&db, "agents").await, 2);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn observer_cleanup_waits_until_its_source_agent_is_removed() {
    let db = database().await;
    let row = observer("agt_a");
    put(&db, COLLECTION, row.clone()).await;
    put(&db, "agents", doc! {"_id":"agt_a"}).await;
    let entry = catalog::ERASURE
        .iter()
        .find(|entry| entry.collection == COLLECTION)
        .unwrap();
    let options = Options {
        transaction_drain_minimum_secs: 120,
        drain_qualification: Some(TransactionDrainQualification::fixture()),
        billing_enabled: false,
        allocation_rollups_enabled: Some(false),
        agent_shared_observation: Some(true),
        retain_legal_consents: false,
    };
    let mut batch = Batch::default();
    let rows = [row];
    let ids = [Bson::String("agt_a".into())];
    batches::remove(
        &db,
        entry,
        batches::DeletePage {
            rows: &rows,
            ids: &ids,
        },
        &[],
        NOW,
        options,
        &mut batch,
    )
    .await
    .unwrap();
    assert!(batch.waiting && batch.has_more);
    assert!(batch.deleted.is_empty());
    assert_eq!(count(&db, COLLECTION).await, 1);
    db.collection::<Document>("agents")
        .delete_one(doc! {"_id":"agt_a"})
        .await
        .unwrap();
    let mut batch = Batch::default();
    batches::remove(
        &db,
        entry,
        batches::DeletePage {
            rows: &rows,
            ids: &ids,
        },
        &[],
        NOW,
        options,
        &mut batch,
    )
    .await
    .unwrap();
    assert_eq!(batch.deleted.get(COLLECTION), Some(&1));
    assert_eq!(count(&db, COLLECTION).await, 0);
    db.drop().await.unwrap();
}

#[test]
fn observer_cleanup_follows_source_deletion() {
    let operations = progress::operations();
    let last_agent = operations
        .iter()
        .rposition(|(entry, _)| entry.collection == "agents")
        .unwrap();
    let first_observer = operations
        .iter()
        .position(|(entry, _)| entry.collection == COLLECTION)
        .unwrap();
    assert!(first_observer > last_agent);
}
