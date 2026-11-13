use super::retained_views::{build, enabled};
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_cleanup_charges_bytes_until_child_deletion_is_acknowledged() {
    let store = enabled().await;
    seed(&store, &[state(0, now() - 86_400)]).await;
    let principal = scope("operator", true);
    let selection = ViewInput::default().validate(now()).unwrap();
    let report = build(&store, &principal, &selection).await;
    let filter = doc! {"_id":&report.report_id};
    let before = store
        .collection(JOBS)
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(before.get("expires_at"), Some(&mongodb::bson::Bson::Null));
    assert!(before.get_i64("reserved_bytes").unwrap() > 0);
    store
        .collection(JOBS)
        .update_one(filter.clone(), doc! {"$set":{"retire_after":now() as i64}})
        .await
        .unwrap();
    store
        .client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":{"times":1},
            "data":{"failCommands":["delete"],"errorCode":2}
        })
        .await
        .unwrap();
    let failed = store.prune_cache(now()).await;
    store
        .client
        .database("admin")
        .run_command(doc! {
            "configureFailPoint":"failCommand","mode":"off"
        })
        .await
        .unwrap();
    assert!(failed.is_err());
    let held = store
        .collection(JOBS)
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.get("expires_at"), Some(&mongodb::bson::Bson::Null));
    assert!(held.get_bool("reserved").unwrap());
    assert_eq!(
        held.get_i64("reserved_bytes").unwrap(),
        before.get_i64("reserved_bytes").unwrap()
    );
    assert!(store.verify_snapshot(&report).await.is_err());
    assert!(
        store
            .collection(ROWS)
            .count_documents(doc! {"report_id":&report.report_id})
            .await
            .unwrap()
            > 0
    );
    store.prune_cache(now()).await.unwrap();
    let released = store
        .collection(JOBS)
        .find_one(filter)
        .await
        .unwrap()
        .unwrap();
    assert!(!released.get_bool("reserved").unwrap());
    assert!(
        released
            .get_datetime("expires_at")
            .unwrap()
            .timestamp_millis()
            >= now() as i64 * 1000
    );
    for collection in [ROWS, CHARTS] {
        assert_eq!(
            store
                .collection(collection)
                .count_documents(doc! {"report_id":&report.report_id})
                .await
                .unwrap(),
            0
        );
    }
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_publication_requires_current_pointer_and_complete_replacement() {
    let store = enabled().await;
    seed(&store, &[state(0, now() - 86_400)]).await;
    let principal = scope("operator", true);
    let mut selection = ViewInput::default().validate(now()).unwrap();
    let original = build(&store, &principal, &selection).await;
    selection.refresh = true;
    store.latest(&principal, &selection, now()).await.unwrap();
    let lease = store
        .claim_report("report:0", "replacement")
        .await
        .unwrap()
        .unwrap();
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap();
    let key = selection.key(&principal).unwrap();
    store
        .collection(CONTROL)
        .update_one(doc! {"_id":&key}, doc! {"$inc":{"revision":1_i64}})
        .await
        .unwrap();
    assert!(store
        .finish_report(&lease, generated.clone(), true)
        .await
        .is_err());
    let pointer = store
        .collection(CONTROL)
        .find_one(doc! {"_id":&key})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pointer.get_str("latest_id").unwrap(), original.report_id);
    let candidate = store
        .collection(JOBS)
        .find_one(doc! {"_id":&lease.report.report_id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(candidate.get_str("state").unwrap(), "view_running");
    // Restore the test mutation, then remove a required child. The publication
    // transaction still must refuse and leave the old complete namespace intact.
    store
        .collection(CONTROL)
        .update_one(doc! {"_id":&key}, doc! {"$inc":{"revision":-1_i64}})
        .await
        .unwrap();
    store
        .collection(CHARTS)
        .delete_one(doc! {"report_id":&lease.report.report_id})
        .await
        .unwrap();
    assert!(store.finish_report(&lease, generated, true).await.is_err());
    store.reauthorize(&original).await.unwrap();
    store
        .finish_report(&lease, lease.report.clone(), false)
        .await
        .unwrap();
    selection.refresh = false;
    let still_ready = store.latest(&principal, &selection, now()).await.unwrap();
    assert_eq!(still_ready.report.report_id, original.report_id);
    store.db.drop().await.unwrap();
}
