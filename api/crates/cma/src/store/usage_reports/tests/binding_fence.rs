use super::*;
use super::{control::stores, read_models::binding};

async fn counter(store: &ReportingStore) -> i64 {
    store
        .collection(CONTROL)
        .find_one(doc! {"_id":"observer"})
        .await
        .unwrap()
        .unwrap()
        .get_i64("control_write_fence")
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn binding_transaction_fences_expired_observer_and_preserves_mapping_revision() {
    let (first, second) = stores().await;
    let at = now() - 1000;
    let home = binding("sandbox", BindingKind::Home, at);
    assert!(first.lease("observer", "original", 70).await.unwrap());
    first
        .reconcile_bindings("original", "sandbox", vec![home.clone()], at)
        .await
        .unwrap();
    assert_eq!(counter(&first).await, 1);
    let revision = first.mapping_revision().await.unwrap();
    first
        .reconcile_bindings("original", "sandbox", vec![home.clone()], at + 60)
        .await
        .unwrap();
    assert_eq!(counter(&second).await, 2);
    assert_eq!(second.mapping_revision().await.unwrap(), revision);
    // Old readers decode only the existing record/version; additive authority
    // bookkeeping cannot alter progress or externally visible mapping versions.
    let progress = Progress {
        from: 12,
        ..Default::default()
    };
    first
        .save_progress("observer", "original", &progress)
        .await
        .unwrap();
    assert_eq!(second.progress("observer").await.unwrap().from, 12);
    first
        .collection(CONTROL)
        .update_one(
            doc! {"_id":"observer"},
            doc! {"$set":{"lease_until":(now() as i64)-1}},
        )
        .await
        .unwrap();
    assert!(first
        .reconcile_bindings("original", "sandbox", vec![], at + 61)
        .await
        .is_err());
    assert!(first
        .expire_missing_bindings("original", at + 200)
        .await
        .is_err());
    assert_eq!(counter(&first).await, 2);
    assert_eq!(second.mapping_revision().await.unwrap(), revision);
    assert!(second.lease("observer", "successor", 70).await.unwrap());
    assert!(first
        .reconcile_bindings("original", "sandbox", vec![], at + 61)
        .await
        .is_err());
    second
        .reconcile_bindings("successor", "sandbox", vec![home], at + 61)
        .await
        .unwrap();
    assert_eq!(counter(&first).await, 3);
    second.release("observer", "successor").await.unwrap();
    first.initialize().await.unwrap();
    assert_eq!(
        counter(&first).await,
        3,
        "release/reinitialize cannot reset the fence"
    );
    assert!(first.lease("observer", "restart", 70).await.unwrap());
    first
        .expire_missing_bindings("restart", at + 200)
        .await
        .unwrap();
    assert_eq!(counter(&first).await, 4);
    assert_eq!(second.mapping_revision().await.unwrap(), revision + 1);
    first.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn observer_takeover_after_transaction_snapshot_refuses_its_commit() {
    let (first, second) = stores().await;
    assert!(first.lease("observer", "original", 70).await.unwrap());
    let mut session = first.client.start_session().await.unwrap();
    session.start_transaction().await.unwrap();
    first
        .collection(CONTROL)
        .find_one(doc! {"_id":"observer"})
        .session(&mut session)
        .await
        .unwrap()
        .unwrap();
    // Stage a mapping effect before a different independent client takes over.
    let rows = first.collection(super::super::super::sandbox_metric_bindings::COLLECTION);
    rows.insert_one(doc! {"_id":"uncommitted-observation","document_schema_version":1_i64})
        .session(&mut session)
        .await
        .unwrap();
    first.release("observer", "original").await.unwrap();
    assert!(second.lease("observer", "successor", 70).await.unwrap());
    assert!(first
        .fence_control_transaction("observer", "original", &mut session)
        .await
        .is_err());
    session.abort_transaction().await.unwrap();
    assert!(rows
        .find_one(doc! {"_id":"uncommitted-observation"})
        .await
        .unwrap()
        .is_none());
    assert!(!first.lease("observer", "competitor", 70).await.unwrap());
    first.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn observer_counter_corruption_and_overflow_refuse_all_mapping_writes() {
    let (first, second) = stores().await;
    assert!(first.lease("observer", "original", 70).await.unwrap());
    let at = now() - 1000;
    let home = binding("sandbox", BindingKind::Home, at);
    for value in [
        mongodb::bson::Bson::Int64(i64::MAX),
        mongodb::bson::Bson::Int64(-1),
        mongodb::bson::Bson::String("invalid".into()),
    ] {
        first
            .collection(CONTROL)
            .update_one(
                doc! {"_id":"observer"},
                doc! {"$set":{"control_write_fence":value}},
            )
            .await
            .unwrap();
        assert!(first
            .reconcile_bindings("original", "sandbox", vec![home.clone()], at)
            .await
            .is_err());
        assert_eq!(second.mapping_revision().await.unwrap(), 0);
        assert!(second
            .bindings(
                &["sandbox".into()],
                Range {
                    from: at,
                    to: at + 500
                }
            )
            .await
            .unwrap()
            .is_empty());
    }
    first.db.drop().await.unwrap();
}
