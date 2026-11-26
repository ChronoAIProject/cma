use super::*;
use crate::store::migrations::usage_projections::{self, COLLECTION};
use cma_am_reporting::ProjectionBatch;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn aggregate_projection_takeover_and_budget_fallback_release_reservation() {
    let store = fixture().await;
    for (collection, index) in usage_projections::inspect(&store.db).await.unwrap() {
        store
            .db
            .run_command(doc! {"createIndexes":collection,"indexes":[index]})
            .await
            .unwrap();
    }
    let at = now();
    let report = store
        .submit(
            scope("operator", true),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let old = store
        .claim_report("report:0", "former-worker")
        .await
        .unwrap()
        .unwrap();
    let mut batch = ProjectionBatch::new(32_768);
    batch
        .push(doc! {
            "report_id":&report.report_id,"generation":old.generation,
            "document_schema_version":1_i64,"kind":"contributor",
            "metric":0_i64,"series":"owner-1","chunk":0_i64,
            "contributor":"owner-1"
        })
        .unwrap();
    let documents = batch.into_documents();
    let bytes = documents
        .iter()
        .map(|row| row.as_bytes().len() as i64)
        .sum();
    assert!(store
        .persist_projections(&old, &documents, bytes, (0, 0))
        .await
        .unwrap());

    let job_filter = doc! {"_id":&report.report_id};
    store
        .collection(JOBS)
        .update_one(job_filter.clone(), doc! {"$set":{"lease_until":0_i64}})
        .await
        .unwrap();
    let next = store
        .claim_report("report:1", "next-worker")
        .await
        .unwrap()
        .unwrap();
    assert!(next.generation > old.generation);
    assert!(store
        .persist_projections(&old, &documents, bytes, (0, 0))
        .await
        .is_err());
    assert_eq!(
        store
            .collection(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    let current = store
        .collection(JOBS)
        .find_one(job_filter.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.get_i64("bytes").unwrap(), 0);
    assert!(!current.contains_key("projection_generation"));

    let mut batch = ProjectionBatch::new(32_768);
    batch
        .push(doc! {
            "report_id":&report.report_id,"generation":next.generation,
            "document_schema_version":1_i64,"kind":"contributor",
            "metric":0_i64,"series":"owner-1","chunk":0_i64,
            "contributor":"owner-1"
        })
        .unwrap();
    let documents = batch.into_documents();
    let bytes = documents
        .iter()
        .map(|row| row.as_bytes().len() as i64)
        .sum();
    let mut tight = (*store).clone();
    tight.budgets.report_bytes = bytes - 1;
    assert!(!tight
        .persist_projections(&next, &documents, bytes, (0, 0))
        .await
        .unwrap());
    let current = store
        .collection(JOBS)
        .find_one(job_filter.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.get_i64("bytes").unwrap(), 0);
    assert!(!current.contains_key("projection_generation"));
    store
        .finish_report(&next, next.report.clone(), true)
        .await
        .unwrap();
    store
        .collection(JOBS)
        .update_one(
            job_filter.clone(),
            doc! {"$set":{"logical_expires_at":0_i64}},
        )
        .await
        .unwrap();
    store.prune_cache(now()).await.unwrap();
    let pruned = store
        .collection(JOBS)
        .find_one(job_filter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pruned.get_i64("bytes").unwrap(), 0);
    assert!(!pruned.get_bool("reserved").unwrap());
    assert_eq!(
        store
            .collection(COLLECTION)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    store.db.drop().await.unwrap();
}
