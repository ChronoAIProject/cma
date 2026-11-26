use super::*;
use crate::store::sandbox_usage_export::delivery::compatibility_tests::previous_roundtrip;
use futures_util::TryStreamExt;
use mongodb::bson::{Bson, DateTime};

async fn previous_reader_write(rows: &Collection<Document>, row: &Document) {
    let current = row.get("delivery").unwrap().clone();
    let mut old = previous_roundtrip(current.clone());
    assert_eq!(old, current);
    old.as_document_mut()
        .unwrap()
        .insert("due_at", DateTime::from_millis(0));
    rows.update_one(
        doc! {"_id":row.get_str("_id").unwrap()},
        doc! {"$set":{"delivery":old}},
    )
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn previous_reader_roundtrips_preserve_claims_receipts_and_subsequent_delivery() {
    for legacy_observation in [false, true] {
        let f = Fixture::new(true).await;
        let parent = f.parent("retained-owner").await;
        let capture = f.capture();
        capture
            .project_one(&source_id(&parent, 1), now())
            .await
            .unwrap();
        let rows = f.db.collection::<Document>(OUTBOX);
        let original: Vec<Document> = rows
            .find(doc! {})
            .sort(doc! {"sequence":1})
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        assert_eq!(original.len(), 2);
        let stream = original[0].get_str("stream_id").unwrap();
        let inbox = Inbox::new(&f, false);
        let worker = UsageExporter::for_test(capture.clone(), "a".repeat(64), inbox.clone());
        worker.sweep(&mut String::new()).await.unwrap();
        let legacy = if legacy_observation {
            Bson::Document(doc! {
                "stream_id":stream,"contiguous_sequence":"1","validated_sequence":"1",
                "highest_sequence":"2","head_digest":original[0].get_str("digest").unwrap(),
                "missing_ranges":[],"snapshot":"retained-previous-observation","next_cursor":Bson::Null,
            })
        } else {
            Bson::Null
        };
        rows.update_one(
            doc! {"sequence":1_i64},
            doc! {"$set":{"delivery.cursor":legacy.clone()}},
        )
        .await
        .unwrap();
        let first = rows
            .find_one(doc! {"sequence":1_i64})
            .await
            .unwrap()
            .unwrap();
        previous_reader_write(&rows, &first).await;

        // A fresh zero-head N2 observation cannot replace the previous diagnostic shape.
        *inbox.cursor.lock().unwrap() = Some(observed_cursor(stream, 0, None));
        worker.sweep(&mut String::new()).await.unwrap();
        let second = rows
            .find_one(doc! {"sequence":2_i64})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            second
                .get_document("delivery")
                .unwrap()
                .get_str("status")
                .unwrap(),
            "accepted"
        );
        assert_eq!(
            second.get_document("delivery").unwrap().get("cursor"),
            Some(&Bson::Null)
        );
        previous_reader_write(&rows, &second).await;
        rows.update_one(
            doc! {"sequence":2_i64},
            doc! {"$set":{"delivery.cursor":legacy.clone()}},
        )
        .await
        .unwrap();

        // Reconciliation checks the fresh validation head without persisting it into v1.
        *inbox.cursor.lock().unwrap() = Some(observed_cursor(
            stream,
            2,
            Some(original[1].get_str("digest").unwrap().into()),
        ));
        worker.sweep(&mut String::new()).await.unwrap();
        let accepted = rows
            .find_one(doc! {"sequence":2_i64})
            .await
            .unwrap()
            .unwrap();
        let delivery = accepted.get_document("delivery").unwrap();
        assert_eq!(delivery.get_str("status").unwrap(), "accepted");
        assert_eq!(delivery.get("cursor"), Some(&legacy));
        let receipt = delivery.get("receipt").unwrap().clone();
        let generation = delivery.get_i64("generation").unwrap();
        previous_reader_write(&rows, &accepted).await;
        previous_reader_write(&rows, &first).await;

        let peer = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let reopened = CaptureStore::open(&peer, f.db.name(), config())
            .await
            .unwrap();
        let resumed = UsageExporter::for_test(reopened.clone(), "a".repeat(64), inbox.clone());
        resumed.sweep(&mut String::new()).await.unwrap();
        let audited = rows
            .find_one(doc! {"sequence":2_i64})
            .await
            .unwrap()
            .unwrap();
        let delivery = audited.get_document("delivery").unwrap();
        assert_eq!(delivery.get_str("status").unwrap(), "accepted");
        assert_eq!(delivery.get_i64("generation").unwrap(), generation + 1);
        assert_eq!(delivery.get("receipt"), Some(&receipt));
        assert_eq!(delivery.get("cursor"), Some(&legacy));
        previous_reader_write(&rows, &audited).await;
        assert_eq!(inbox.submissions.lock().unwrap().len(), 2);

        f.store
            .checkpoint_usage_one(parent.id.as_str(), MINUTE)
            .await
            .unwrap();
        assert!(reopened
            .project_one(&source_id(&parent, 2), now())
            .await
            .unwrap());
        assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 3);
        resumed.sweep(&mut String::new()).await.unwrap();
        let next = rows
            .find_one(doc! {"sequence":3_i64})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            next.get_document("delivery")
                .unwrap()
                .get_str("status")
                .unwrap(),
            "accepted"
        );
        previous_reader_write(&rows, &next).await;
        assert_eq!(inbox.submissions.lock().unwrap().len(), 3);
        for before in &original {
            let mut after = rows
                .find_one(doc! {"_id":before.get_str("_id").unwrap()})
                .await
                .unwrap()
                .unwrap();
            let delivery = after.remove("delivery").unwrap();
            assert_eq!(previous_roundtrip(delivery.clone()), delivery);
            assert_eq!(&after, before);
        }
        f.db.drop().await.unwrap();
    }
}
