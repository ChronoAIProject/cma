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
            "data":{"failCommands":["delete"],"errorCode":50}
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
    let fresh = store
        .latest(&scope("foreign", true), &selection, now())
        .await
        .unwrap()
        .report;
    let lease = store
        .claim_report("report:1", "unrelated-report")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.report.report_id, fresh.report_id);
    let generated =
        crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
            .await
            .unwrap();
    store.finish_report(&lease, generated, true).await.unwrap();
    assert!(
        store
            .report(&scope("foreign", true), &fresh.report_id, now())
            .await
            .unwrap()
            .report_complete
    );

    // While the failed worker's cleanup lease is still held, another worker
    // cannot immediately retry the same delete.
    store.prune_cache(now()).await.unwrap();
    assert!(store
        .collection(JOBS)
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap()
        .get_bool("reserved")
        .unwrap());
    store
        .collection(CONTROL)
        .update_one(
            doc! {"_id":super::super::retirement::SLOT},
            doc! {"$set":{"lease_until":0_i64}},
        )
        .await
        .unwrap();
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
async fn retirement_resumes_bounded_batches_and_rotates_to_another_parent() {
    let store = fixture().await;
    let at = now();
    let query = ReportInput::default().validate(at).unwrap();
    let a = store
        .submit(scope("first", true), query.clone(), at)
        .await
        .unwrap();
    let b = store
        .submit(scope("second", true), query, at)
        .await
        .unwrap();
    let pending = doc! {"_id":{"$in":[&a.report_id,&b.report_id]}};
    store
        .collection(JOBS)
        .update_many(
            pending,
            doc! {"$set":{"logical_expires_at":0_i64,"expires_at":expires(at + 3600)}},
        )
        .await
        .unwrap();
    let (first, other) = if a.report_id < b.report_id {
        (a, b)
    } else {
        (b, a)
    };
    let first_job = doc! {"_id":&first.report_id};
    let first_rows = doc! {"report_id":&first.report_id};
    store
        .collection(ROWS)
        .insert_many((0..130).map(|index| {
            doc! {
                "_id":format!("retirement-row-{index}"),"document_schema_version":1_i64,
                "report_id":&first.report_id,"sandbox_id":format!("sandbox-{index}"),
            }
        }))
        .await
        .unwrap();
    store.prune_cache(at).await.unwrap();
    assert_eq!(
        store
            .collection(ROWS)
            .count_documents(first_rows.clone())
            .await
            .unwrap(),
        66
    );
    let held = store
        .collection(JOBS)
        .find_one(first_job.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(held.get_bool("reserved").unwrap());
    assert_eq!(held.get("expires_at"), Some(&mongodb::bson::Bson::Null));
    assert!(store
        .report(&first.scope, &first.report_id, now())
        .await
        .is_err());

    let second_store = ReportingStore::connect(
        &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        store.db.name(),
        &[19; 32],
    )
    .await
    .unwrap();
    second_store.prune_cache(at).await.unwrap();
    let completed = store
        .collection(JOBS)
        .find_one(doc! {"_id":&other.report_id})
        .await
        .unwrap()
        .unwrap();
    assert!(!completed.get_bool("reserved").unwrap());
    assert_eq!(
        store
            .collection(ROWS)
            .count_documents(first_rows.clone())
            .await
            .unwrap(),
        66
    );

    second_store.prune_cache(at).await.unwrap();
    assert_eq!(
        store
            .collection(ROWS)
            .count_documents(first_rows.clone())
            .await
            .unwrap(),
        2
    );
    second_store.prune_cache(at).await.unwrap();
    assert_eq!(
        store
            .collection(ROWS)
            .count_documents(first_rows)
            .await
            .unwrap(),
        0
    );
    let completed = store
        .collection(JOBS)
        .find_one(first_job)
        .await
        .unwrap()
        .unwrap();
    assert!(!completed.get_bool("reserved").unwrap());
    assert!(completed.get_datetime("expires_at").is_ok());
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

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn stale_cleanup_candidate_cannot_retire_a_reclaimed_ordinary_report() {
    let store = fixture().await;
    let report = store
        .submit(
            scope("operator", true),
            ReportInput::default().validate(now()).unwrap(),
            now(),
        )
        .await
        .unwrap();
    let id = doc! {"_id":&report.report_id};
    let original = store
        .collection(JOBS)
        .find_one(id.clone())
        .await
        .unwrap()
        .unwrap();
    // A delayed expiry scan can retain this document while a claimant advances
    // the generation. The cleanup compare-and-set must reject that old scan.
    store.collection(JOBS).update_one(id.clone(),doc! {"$set":{"logical_expires_at":0_i64,"state":"running"},"$inc":{"generation":1_i64}}).await.unwrap();
    assert!(!store.retire_expired_report(&original).await.unwrap());
    let expired = store
        .collection(JOBS)
        .find_one(id.clone())
        .await
        .unwrap()
        .unwrap();
    store
        .collection(JOBS)
        .update_one(
            id.clone(),
            doc! {"$set":{"logical_expires_at":(now()+600) as i64}},
        )
        .await
        .unwrap();
    assert!(!store.retire_expired_report(&expired).await.unwrap());
    let alive = store
        .collection(JOBS)
        .find_one(id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(alive.get_str("state").unwrap(), "running");
    assert!(alive.get_bool("reserved").unwrap());
    store
        .collection(JOBS)
        .update_one(id.clone(), doc! {"$set":{"logical_expires_at":0_i64}})
        .await
        .unwrap();
    let expired = store
        .collection(JOBS)
        .find_one(id.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(store.retire_expired_report(&expired).await.unwrap());
    assert_eq!(
        store
            .collection(JOBS)
            .find_one(id)
            .await
            .unwrap()
            .unwrap()
            .get_str("state")
            .unwrap(),
        "retired"
    );
    store.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn report_chunk_budget_does_not_replace_server_time_expiry_fence() {
    let store = enabled().await;
    store
        .latest(
            &scope("operator", true),
            &ViewInput::default().validate(now()).unwrap(),
            now(),
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "expired-chunk")
        .await
        .unwrap()
        .unwrap();
    let id = doc! {"_id":&lease.report.report_id};
    store
        .collection(JOBS)
        .update_one(id.clone(), doc! {"$set":{"lease_until":0_i64}})
        .await
        .unwrap();
    assert!(store.write_report_chunk(&lease, &[], vec![]).await.is_err());
    assert!(store
        .finish_report(&lease, lease.report.clone(), false)
        .await
        .is_err());
    let job = store.collection(JOBS).find_one(id).await.unwrap().unwrap();
    assert_eq!(job.get_str("state").unwrap(), "view_running");
    assert_eq!(job.get_i64("bytes").unwrap(), 0);
    assert_eq!(job.get_i64("row_count").unwrap(), 0);
    assert_eq!(job.get_i64("chart_count").unwrap(), 0);
    store.db.drop().await.unwrap();
}
