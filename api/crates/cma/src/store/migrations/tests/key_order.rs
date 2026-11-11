use super::*;
use futures_util::TryStreamExt;

fn reordered(index: &Document) -> Document {
    let mut changed = index.clone();
    let mut keys: Vec<_> = index
        .get_document("key")
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    keys.rotate_left(1);
    changed.insert("key", keys.into_iter().collect::<Document>());
    changed
}

#[test]
fn compound_key_order_is_required_and_differently_named_orders_are_distinct() {
    for position in 0..2 {
        let expected = activity_indexes::indexes();
        let mut existing = expected.to_vec();
        existing[position] = reordered(&existing[position]);
        assert!(activity_indexes::missing(&existing).is_err());
        existing[position].insert("name", "different-query-order");
        let pending = activity_indexes::missing(&existing).unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0]
            .get_document("key")
            .unwrap()
            .iter()
            .eq(expected[position].get_document("key").unwrap().iter()));
        existing.push(expected[position].clone());
        assert!(activity_indexes::missing(&existing).unwrap().is_empty());
    }
}

async fn index_bytes(database: &mongodb::Database) -> Vec<u8> {
    let response = database
        .run_command(doc! {"listIndexes":"activity"})
        .await
        .unwrap();
    let indexes = response
        .get_document("cursor")
        .unwrap()
        .get_array("firstBatch")
        .unwrap();
    mongodb::bson::serialize_to_vec(&doc! {"indexes":indexes.clone()}).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn reordered_indexes_refuse_plan_apply_and_completed_postchecks() {
    for completed in [false, true] {
        for position in 0..2 {
            let (_, database, ledger) = fixture().await;
            let records = database.collection::<Document>("activity");
            let saved = doc! {"_id":"retained","record":{"actor":"fixture","action":"unchanged"}};
            records.insert_one(saved.clone()).await.unwrap();
            let indexes = activity_indexes::indexes();
            if completed {
                apply_indexes(&ledger, &database, &identity())
                    .await
                    .unwrap();
                // Introduce drift only in this disposable fixture, after a valid receipt.
                records
                    .drop_index(indexes[position].get_str("name").unwrap())
                    .await
                    .unwrap();
            } else {
                activity_indexes::create(&database, indexes[1 - position].clone())
                    .await
                    .unwrap();
            }
            activity_indexes::create(&database, reordered(&indexes[position]))
                .await
                .unwrap();
            let before = index_bytes(&database).await;
            let receipts = database.collection::<Document>(ledger::COLLECTION);
            let prior: Vec<Document> = receipts
                .find(doc! {"kind":"attempt","outcome":"completed"})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            assert_eq!(prior.len(), usize::from(completed));
            // This inspection is the read-only plan's index preflight.
            assert!(activity_indexes::inspect(&database).await.is_err());
            assert!(apply_indexes(&ledger, &database, &identity())
                .await
                .is_err());
            assert_eq!(index_bytes(&database).await, before);
            let after: Vec<Document> = receipts
                .find(doc! {"kind":"attempt","outcome":"completed"})
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            assert_eq!(after, prior);
            assert_eq!(ledger.completed().await.unwrap(), completed);
            assert_eq!(
                records.find_one(doc! {"_id":"retained"}).await.unwrap(),
                Some(saved)
            );
            database.drop().await.unwrap();
        }
    }
}
