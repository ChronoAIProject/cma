use super::*;

#[tokio::test]
#[ignore = "requires disposable Mongo contract runner with failpoints"]
async fn aggregate_projection_reconciles_committed_manifest_after_lost_acknowledgement() {
    use crate::store::migrations::usage_projections::{self, COLLECTION};
    use cma_am_reporting::ProjectionBatch;

    let f = Fixture::new().await;
    for (collection, index) in usage_projections::inspect(&f.store.db).await.unwrap() {
        f.store
            .db
            .run_command(doc! {"createIndexes":collection,"indexes":[index]})
            .await
            .unwrap();
    }
    let at = now();
    f.store
        .submit(
            scope("operator", true),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = f
        .store
        .claim_report("report:0", "aggregation-worker")
        .await
        .unwrap()
        .unwrap();
    let build = |order: [i64; 2], gain: i64| {
        let mut batch = ProjectionBatch::new(32_768);
        for chunk in order {
            batch
                .push(
                    doc! {"report_id":&lease.report.report_id,"generation":lease.generation,
                    "kind":"chart_total","metric":0_i64,"series":"","chunk":chunk,"gain":gain},
                )
                .unwrap();
        }
        let docs = batch.into_documents();
        let bytes = docs.iter().map(|row| row.as_bytes().len() as i64).sum();
        (docs, bytes)
    };
    let (initial, bytes) = build([0, 1], 7);
    f.commands.clear();
    f.fail(&["commitTransaction"], 20, doc! {
        "writeConcernError":{"code":50,"codeName":"MaxTimeMSExpired","errmsg":"injected commit uncertainty"}
    }).await;
    let outcome = f
        .store
        .persist_projections(&lease, &initial, bytes, (0, 0))
        .await;
    f.disable().await;
    assert!(outcome.unwrap());
    assert_eq!(f.commands.attempts(), 1);
    assert_eq!(f.commands.count("commitTransaction"), 5);

    let (replay, replay_bytes) = build([1, 0], 7);
    assert!(f
        .store
        .persist_projections(&lease, &replay, replay_bytes, (0, 0))
        .await
        .unwrap());
    let (amended, amended_bytes) = build([1, 0], 8);
    assert!(matches!(
        f.store
            .persist_projections(&lease, &amended, amended_bytes, (0, 0))
            .await,
        Err(MetricsError::Unavailable)
    ));
    let job = f
        .store
        .collection(JOBS)
        .find_one(doc! {"_id":&lease.report.report_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.get_i64("bytes").unwrap(), bytes);
    let persisted = f
        .store
        .collection(COLLECTION)
        .count_documents(doc! {"report_id":&lease.report.report_id})
        .await
        .unwrap();
    assert_eq!(persisted, 2);
    f.store.db.drop().await.unwrap();
}
