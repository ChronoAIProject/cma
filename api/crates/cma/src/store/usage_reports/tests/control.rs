use super::*;

async fn stores() -> (Arc<ReportingStore>, Arc<ReportingStore>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let name = format!("report_control_{}", uuid::Uuid::new_v4().simple());
    // Independent client pools and circuits; only the durable control rows are shared.
    let first = ReportingStore::connect(&uri, &name, &[19; 32])
        .await
        .unwrap();
    let second = ReportingStore::connect(&uri, &name, &[19; 32])
        .await
        .unwrap();
    (first, second)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn reporting_control_excludes_overlap_and_fences_delayed_cycle_release() {
    let (first, second) = stores().await;
    let key = "observer";
    let (a, b) = tokio::join!(first.lease(key, "a", 70), second.lease(key, "b", 70));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a, b, "only one independent worker may acquire the slot");
    let original = if a { "a" } else { "b" };
    assert!(!first.lease(key, original, 70).await.unwrap());
    first
        .save_progress(
            key,
            original,
            &Progress {
                from: 12,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    first.release(key, original).await.unwrap();
    assert!(second.lease(key, "next-cycle", 70).await.unwrap());
    assert!(first.release(key, original).await.is_err());
    assert!(first
        .save_progress(key, original, &Progress::default())
        .await
        .is_err());
    assert_eq!(second.progress(key).await.unwrap().from, 12);
    assert!(!first.lease(key, "competitor", 70).await.unwrap());
    second
        .save_progress(
            key,
            "next-cycle",
            &Progress {
                from: 24,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    second.release(key, "next-cycle").await.unwrap();
    assert_eq!(first.progress(key).await.unwrap().from, 24);
    first.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn reporting_control_expiry_refuses_late_writes_and_allows_takeover() {
    let (first, second) = stores().await;
    let key = "replay";
    assert!(first.lease(key, "crashed", 70).await.unwrap());
    first
        .collection(CONTROL)
        .update_one(
            doc! {"_id":key},
            doc! {"$set":{"lease_until":(now() as i64)-1}},
        )
        .await
        .unwrap();
    assert!(first.release(key, "crashed").await.is_err());
    assert!(first
        .save_progress(key, "crashed", &Progress::default())
        .await
        .is_err());
    assert!(second.lease(key, "successor", 70).await.unwrap());
    assert!(!first.lease(key, "crashed", 70).await.unwrap());
    assert!(first.release(key, "crashed").await.is_err());
    second
        .save_progress(key, "successor", &Progress::default())
        .await
        .unwrap();
    second.release(key, "successor").await.unwrap();
    assert!(first.lease(key, "restart", 70).await.unwrap());
    first.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn reporting_control_rejects_invalid_duration_or_identity_without_writes() {
    let (first, second) = stores().await;
    for (key, holder, seconds) in [
        ("observer", "", 70),
        ("", "a", 70),
        ("observer", "a", 0),
        ("observer", "a", u64::MAX),
    ] {
        assert!(first.lease(key, holder, seconds).await.is_err());
    }
    assert!(!first.lease("unknown-task", "a", 70).await.unwrap());
    assert!(second.lease("observer", "valid", 70).await.unwrap());
    second.release("observer", "valid").await.unwrap();
    first.db.drop().await.unwrap();
}
