use super::*;
use super::{fixture::*, observation::Observation};
use std::{
    future::{poll_fn, Future},
    task::Poll,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn cancellation_at_cooperative_yield_aborts_transaction_and_counts_consumed_work() {
    let observation = Observation::default();
    let client = observation.client().await;
    let db = database(&client);
    let collection = db.collection::<Document>("agent_log");
    let writes = db.collection::<Document>("pending_writes");
    writes.insert_one(doc! {"_id": "committed"}).await.unwrap();
    let id = AgentId("agt_cancel_fixture".into());
    let entries = entries(&id, 200, 128);
    insert(&collection, &entries).await;
    let consumed = entries[..8]
        .iter()
        .map(|entry| log_document(entry).unwrap().to_vec().unwrap().len() as u64)
        .sum::<u64>();
    observation.begin();
    let visits = std::cell::Cell::new(0);
    let (_, execution) = isolated_test(recorded_execution(async {
        // Session ownership stays inside the cancelled operation, just as in a
        // transaction caller. Driver session Drop schedules abortTransaction.
        let mut operation = Box::pin(async {
            let mut session = client.start_session().await.unwrap();
            session.start_transaction().await.unwrap();
            writes
                .insert_one(doc! {"_id": "must-not-commit"})
                .session(&mut session)
                .await
                .unwrap();
            scan_collection(
                ScanCaller::Other,
                &collection,
                &id,
                200,
                &mut session,
                options(true),
                |_| {
                    visits.set(visits.get() + 1);
                    Ok(())
                },
            )
            .await
            .unwrap();
            session.commit_transaction().await.unwrap();
        });
        poll_fn(|cx| {
            assert!(
                operation.as_mut().poll(cx).is_pending(),
                "scan finished before cancellation"
            );
            if visits.get() >= 8 {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        assert_eq!(visits.get(), 8);
        drop(operation);
        assert_eq!(&snapshot(ScanCaller::Other, false)[..3], &[1, 8, consumed]);
        assert_eq!(&snapshot(ScanCaller::Other, true)[..3], &[0, 0, 0]);
    }))
    .await;
    assert_eq!(execution.len(), 1);
    assert_eq!(execution[0].1, 1);
    // A bounded wait observes protocol cleanup, with no latency assertion.
    tokio::time::timeout(Duration::from_secs(30), async {
        while observation.commands("abortTransaction") == 0
            || observation.commands("killCursors") == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(observation.commands("commitTransaction"), 0);
    assert!(writes
        .find_one(doc! {"_id": "must-not-commit"})
        .await
        .unwrap()
        .is_none());
    assert_eq!(writes.count_documents(doc! {}).await.unwrap(), 1);
    db.drop().await.unwrap();
}
