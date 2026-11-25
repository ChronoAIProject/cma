use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn account_erasure_preserves_mixed_allocated_and_measured_financial_records() {
    use crate::store::account_erasure::{erase_records, Cursor, Target};
    let f = Fixture::new().await;
    f.bind().await;
    let lease = f.lease().await;
    let publication = f.input(&lease).await.freeze(f.samples(60.0), true).unwrap();
    f.store
        .publish_measured(&lease, &publication)
        .await
        .unwrap();
    f.store.release_measured(&lease).await.unwrap();
    f.db.collection::<Document>("users")
        .insert_one(doc! {"_id":&f.stream.owner})
        .await
        .unwrap();
    let names = super::super::super::COLLECTIONS;
    let mut before = Vec::new();
    for name in names {
        before.push((*name, f.rows(name).await));
    }
    let subject = layer2_core::SubjectId::new(&f.stream.owner);
    let inventory = layer2_core::account_deletion::ErasureInventory {
        sandboxes: vec![f.stream.sandbox_id.clone()],
        ..Default::default()
    };
    let mut cursor = Cursor::default();
    let mut complete = false;
    for _ in 0..2000 {
        let batch = erase_records(
            &f.db,
            &[Target {
                subject: &subject,
                inventory: &inventory,
            }],
            &mut cursor,
            &layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
        for name in names {
            assert!(!batch.deleted.contains_key(*name));
            assert!(!batch.redacted.contains_key(*name));
        }
        if !batch.has_more {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert!(f.rows("users").await.is_empty());
    assert!(f.rows(sandbox_metric_bindings::COLLECTION).await.is_empty());
    for (name, rows) in before {
        assert_eq!(f.rows(name).await, rows, "{name}");
    }
    let request = super::super::repair::MeasuredRepair {
        source_stream: f.stream.id.clone(),
        minute: f.minute,
        evidence_digest: publication.digest,
        predecessor_event: publication.event_id,
    };
    assert!(f.store.repair_measured(&request, true).await.unwrap());
    assert!(f.rows("users").await.is_empty());
    f.db.drop().await.unwrap();
}
