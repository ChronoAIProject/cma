use super::mongo::{checkpoint, fixture, progress, ready, seed};
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unknown_commit_reconciles_buckets_and_progress_before_retry() {
    let store = fixture().await;
    let lease = store
        .claim_allocation_buckets(Lane::Backfill)
        .await
        .unwrap()
        .unwrap();
    let from = (lease.at / DAY - 1) * DAY;
    let b = materialize::build(
        &state(1, from),
        &[],
        Range {
            from,
            to: from + DAY,
        },
    )
    .unwrap();
    let admin = store.client.database("admin");
    admin
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":{"times":5},
            "data":{"failCommands":["commitTransaction"],"writeConcernError":{
                "code":50,"codeName":"MaxTimeMSExpired","errmsg":"fixture uncertain publication"}}
        })
        .await
        .unwrap();
    let outcome = store
        .publish_allocation_buckets(&lease, std::slice::from_ref(&b), &progress(from))
        .await;
    admin
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(outcome.is_err());
    crate::store::test_visibility::majority_barrier(&store.client, store.db.name()).await;
    let saved = store
        .collection(COLLECTION)
        .find_one(doc! {"_id":b.key()})
        .await
        .unwrap()
        .unwrap();
    assert_projection(
        &DayBucket::read(&saved).unwrap().total().projection,
        &b.total().projection,
    );
    assert!(checkpoint(&store, Lane::Backfill)
        .await
        .contains_key("record"));
    assert!(store
        .publish_allocation_buckets(&lease, &[b], &progress(from))
        .await
        .is_err());
    ready(&store, Lane::Backfill).await;
    let fresh = store
        .claim_allocation_buckets(Lane::Backfill)
        .await
        .unwrap()
        .unwrap();
    assert!(fresh.progress.unwrap().pass_complete);
    assert_eq!(
        store
            .collection(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn large_pages_shrink_to_transaction_budget_without_skipping_members() {
    const EVENT_SECONDS: u64 = 10;
    let store = fixture().await;
    let from = (super::super::super::usage_reports::now() / DAY - 1) * DAY;
    let page = worker::PAGE_SIZE as usize;
    let states: Vec<_> = (1..=page)
        .map(|id| {
            let mut s = state(id as u64, from);
            s.revision = DAY / EVENT_SECONDS;
            s.source_seq = s.revision;
            s.source_revision = s.revision;
            s.last_observed_at = from + DAY;
            s
        })
        .collect();
    seed(&store, &states).await;
    let mut page_document_bytes = 0;
    for s in &states {
        let events: Vec<_> = (0..DAY / EVENT_SECONDS)
            .map(|n| {
                let mut e = event(
                    s,
                    n + 1,
                    from + n * EVENT_SECONDS,
                    from + (n + 1) * EVENT_SECONDS,
                );
                e.allocation.running = n % 2 == 0;
                e.quantity = e.allocation.usage(EVENT_SECONDS).unwrap();
                e
            })
            .collect();
        let bucket = materialize::build_covered(
            s,
            &events,
            Range {
                from,
                to: from + DAY,
            },
            from + DAY,
        )
        .unwrap();
        page_document_bytes += mongodb::bson::serialize_to_vec(&bucket.document().unwrap())
            .unwrap()
            .len();
        let documents: Vec<_> = events
            .iter()
            .map(|e| super::super::super::sandbox_usage::event_document(e).unwrap())
            .collect();
        store
            .collection(super::super::super::sandbox_usage::EVENTS)
            .insert_many(documents)
            .await
            .unwrap();
    }
    // Keep exercising adaptive shrinking even with the smaller normal page.
    // Each bucket must encode successfully while the full page exceeds 4 MiB.
    assert!(page_document_bytes > 4 * 1024 * 1024);
    let started = std::time::Instant::now();
    let first = store
        .maintain_allocation_buckets(true, Lane::Backfill)
        .await
        .unwrap();
    assert!(first.written > 0 && first.written < page);
    assert_eq!(first.states, first.written);
    let mut written = first.written;
    while written < page {
        ready(&store, Lane::Backfill).await;
        let next = store
            .maintain_allocation_buckets(true, Lane::Backfill)
            .await
            .unwrap();
        assert!(next.written > 0);
        written += next.written;
    }
    assert_eq!(written, page);
    assert_eq!(
        store
            .collection(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        page as u64
    );
    eprintln!(
        "allocation adaptive pages members={page} first_page={} events={} elapsed_ms={}",
        first.written,
        page as u64 * DAY / EVENT_SECONDS,
        started.elapsed().as_millis()
    );
    store.db.drop().await.unwrap();
}
