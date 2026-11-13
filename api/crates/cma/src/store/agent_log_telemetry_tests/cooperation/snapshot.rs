//! A suspended scan keeps its original view across getMore and concurrent writes.
use super::fixture::*;
use super::*;
use std::{
    cell::Cell,
    future::{poll_fn, Future},
    task::Poll,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn raw_batches_preserve_snapshot_and_transaction_views_across_get_more() {
    let client = client().await;
    let db = database(&client);
    let collection = db.collection::<Document>("agent_log");
    let id = AgentId("agt_snapshot_fixture".into());
    let expected = entries(&id, 200, 128);
    let mut summaries = Vec::new();
    for snapshot in [false, true] {
        for raw_batches in [false, true] {
            collection.delete_many(doc! {}).await.unwrap();
            insert(&collection, &expected).await;
            let mut session = client.start_session().snapshot(snapshot).await.unwrap();
            if !snapshot {
                session
                    .start_transaction()
                    .read_concern(mongodb::options::ReadConcern::snapshot())
                    .await
                    .unwrap();
            }
            let visits = Cell::new(0);
            let mut visited = Vec::new();
            let mut scan = Box::pin(scan_collection(
                ScanCaller::Other,
                &collection,
                &id,
                200,
                &mut session,
                ScanOptions::new(snapshot, Policy::enabled(true))
                    .with_clock(StepClock::new())
                    .with_raw_batches(raw_batches),
                |entry| {
                    visits.set(visits.get() + 1);
                    visited.push(entry);
                    Ok(())
                },
            ));
            poll_fn(|cx| {
                assert!(scan.as_mut().poll(cx).is_pending());
                if visits.get() >= 8 {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            assert_eq!(visits.get(), 8);
            // Row 150 is outside the initial 101-row batch; the new row must
            // also remain invisible to the cursor's established snapshot.
            let mut changed = expected[149].clone();
            changed.kind = AgentLogKind::AgentMessage {
                content: "changed".into(),
            };
            collection
                .replace_one(doc! {"seq": 150_i64}, log_document(&changed).unwrap())
                .await
                .unwrap();
            let mut appended = expected[199].clone();
            appended.seq = 201;
            insert(&collection, &[appended]).await;
            summaries.push(scan.await.unwrap());
            assert_eq!(visited, expected);
            assert_eq!(collection.count_documents(doc! {}).await.unwrap(), 201);
            if !snapshot {
                session.commit_transaction().await.unwrap();
            }
        }
    }
    assert!(summaries.iter().all(|summary| summary == &summaries[0]));
    db.drop().await.unwrap();
}
