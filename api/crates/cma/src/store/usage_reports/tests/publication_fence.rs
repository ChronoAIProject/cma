use super::*;
use super::{control::stores, read_models::binding};
use crate::store::sandbox_metric_rollups::{LiveReading, COLLECTION as METRICS};

fn rollup() -> MetricRollup {
    let hour = now() / 3600 * 3600 - 3600;
    MetricRollup {
        schema_version: 1,
        algorithm_version: ALGORITHM_VERSION,
        source: SOURCE.into(),
        sandbox_id: "sandbox".into(),
        owner_subject: "owner".into(),
        day: hour / 86_400 * 86_400,
        chunk: ((hour % 86_400) / 3600) as u16,
        mapping_revision: 0,
        cutoff: hour + 60,
        source_complete: true,
        minutes: vec![],
    }
}

async fn expire(store: &ReportingStore, key: &str) {
    store
        .collection(CONTROL)
        .update_one(
            doc! {"_id":key},
            doc! {"$set":{"lease_until":now() as i64-1}},
        )
        .await
        .unwrap();
}

async fn rows(store: &ReportingStore, collection: &str, filter: Document) -> Vec<Document> {
    store
        .collection(collection)
        .find(filter)
        .sort(doc! {"_id":1})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn expired_rollup_and_live_publishers_cannot_overwrite_successor_data() {
    let (first, second) = stores().await;
    assert!(first.lease("rollups", "original", 70).await.unwrap());
    let mut rollup = rollup();
    let mut live = LiveReading {
        owner: "owner".into(),
        observed_at: now(),
        metrics: Default::default(),
    };
    first
        .save_metric_rollups("rollups", "original", &[rollup.clone()])
        .await
        .unwrap();
    first
        .save_live("original", &[("sandbox".into(), live.clone())])
        .await
        .unwrap();
    let before = rows(&second, METRICS, doc! {}).await;
    expire(&first, "rollups").await;
    rollup.cutoff += 60;
    live.observed_at += 1;
    assert!(first
        .save_metric_rollups("rollups", "original", &[rollup.clone()])
        .await
        .is_err());
    assert!(first
        .save_live("original", &[("sandbox".into(), live.clone())])
        .await
        .is_err());
    assert_eq!(rows(&second, METRICS, doc! {}).await, before);
    assert!(second.lease("rollups", "successor", 70).await.unwrap());
    second
        .save_metric_rollups("rollups", "successor", &[rollup.clone()])
        .await
        .unwrap();
    second
        .save_live("successor", &[("sandbox".into(), live.clone())])
        .await
        .unwrap();
    let after = rows(&first, METRICS, doc! {}).await;
    assert_ne!(after, before);
    rollup.cutoff += 60;
    live.observed_at += 1;
    assert!(first
        .save_metric_rollups("rollups", "original", &[rollup])
        .await
        .is_err());
    assert!(first
        .save_live("original", &[("sandbox".into(), live)])
        .await
        .is_err());
    assert_eq!(rows(&second, METRICS, doc! {}).await, after);
    first.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn diagnostic_continuity_and_infrastructure_require_original_live_cycle() {
    let (first, second) = stores().await;
    assert!(first.lease("diagnostics", "original", 70).await.unwrap());
    let mut candidate = binding("", BindingKind::Compute, now());
    candidate.owner_subject = "unassigned_pool".into();
    let mut totals = Infrastructure {
        state: "ok".into(),
        observed_at: Some(now()),
        ..Default::default()
    };
    first
        .diagnostic_bindings("original", vec![candidate.clone()])
        .await
        .unwrap();
    first
        .save_infrastructure("original", &totals, 0)
        .await
        .unwrap();
    let filter =
        doc! {"_id":{"$in":[format!("diagnostic:{}",candidate.physical_key()),"infrastructure"]}};
    let before = rows(&second, CONTROL, filter.clone()).await;
    expire(&first, "diagnostics").await;
    candidate.observed_at += 1;
    totals.unproven_count = 9;
    assert!(first
        .diagnostic_bindings("original", vec![candidate.clone()])
        .await
        .is_err());
    assert!(first
        .save_infrastructure("original", &totals, 0)
        .await
        .is_err());
    assert_eq!(rows(&second, CONTROL, filter.clone()).await, before);
    assert!(second.lease("diagnostics", "successor", 70).await.unwrap());
    second
        .diagnostic_bindings("successor", vec![candidate.clone()])
        .await
        .unwrap();
    second
        .save_infrastructure("successor", &totals, 0)
        .await
        .unwrap();
    let after = rows(&first, CONTROL, filter.clone()).await;
    assert_ne!(after, before);
    assert!(first
        .diagnostic_bindings("original", vec![candidate])
        .await
        .is_err());
    assert!(first
        .save_infrastructure("original", &Infrastructure::default(), 0)
        .await
        .is_err());
    assert_eq!(rows(&second, CONTROL, filter).await, after);
    first.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn control_fence_failure_aborts_all_materialized_writes_in_the_batch() {
    let (first, second) = stores().await;
    assert!(first.lease("replay", "original", 70).await.unwrap());
    let mut a = rollup();
    let mut b = a.clone();
    b.sandbox_id = "second".into();
    for value in [
        mongodb::bson::Bson::Int64(i64::MAX),
        mongodb::bson::Bson::String("invalid".into()),
    ] {
        first
            .collection(CONTROL)
            .update_one(
                doc! {"_id":"replay"},
                doc! {"$set":{"control_write_fence":value}},
            )
            .await
            .unwrap();
        assert!(first
            .save_metric_rollups("replay", "original", &[a.clone(), b.clone()])
            .await
            .is_err());
        assert!(rows(&second, METRICS, doc! {}).await.is_empty());
    }
    first
        .collection(CONTROL)
        .update_one(
            doc! {"_id":"replay"},
            doc! {"$unset":{"control_write_fence":""}},
        )
        .await
        .unwrap();
    first
        .save_metric_rollups("replay", "original", &[a.clone(), b.clone()])
        .await
        .unwrap();
    assert_eq!(rows(&second, METRICS, doc! {}).await.len(), 2);
    let before = rows(&second, METRICS, doc! {}).await;
    a.cutoff += 60;
    b.cutoff += 60;
    let updates = vec![
        doc! {"q":{"sandbox_id":"sandbox"},"u":{"$set":{"cutoff":a.cutoff as i64}},"multi":false},
        doc! {"q":{"sandbox_id":"second"},"u":{"$invalid":{"cutoff":b.cutoff as i64}},"multi":false},
    ];
    assert!(first
        .controlled_updates("replay", "original", METRICS, updates)
        .await
        .is_err());
    assert_eq!(rows(&second, METRICS, doc! {}).await, before);
    first.db.drop().await.unwrap();
}
