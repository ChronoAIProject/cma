//! HTTP requests use the production report adapter and a disposable replica set.
use super::{allocated::request, *};
use serde_json::{json, Value};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn tenant_isolation_report_ids_and_cached_rows_never_cross_principals() {
    let store = fixture().await;
    let from = (now() - 3600) / 60 * 60;
    let own = state(1, from);
    let foreign = state(2, from);
    seed(&store, &[own.clone(), foreign.clone()]).await;
    for admin in [false, true] {
        let prefix = if admin {
            "/api/v1/admin/usage/metrics/reports"
        } else {
            "/api/v1/usage/metrics/reports"
        };
        let (status, pending) = request(
            &store,
            "POST",
            prefix,
            "owner-1",
            admin,
            json!({"from":from,"to":from+60}),
        )
        .await;
        assert_eq!(status, 202);
        let id = pending["report_id"].as_str().unwrap();
        let lease = store
            .claim_report("report:0", "isolation")
            .await
            .unwrap()
            .unwrap();
        let generated =
            crate::sandbox_metrics::generate_fixture_with_collection(store.clone(), &lease, false)
                .await
                .unwrap();
        store.finish_report(&lease, generated, true).await.unwrap();
        // A second adapter proves no process-local principal cache supplies access.
        let reopened = ReportingStore::connect(
            &std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
            store.db.name(),
            &[19; 32],
        )
        .await
        .unwrap();
        let mut suffixes = vec![
            String::new(),
            "/sandboxes?limit=1".into(),
            format!("/sandboxes/{}", own.sandbox_id),
            "/series?metric=cpu".into(),
        ];
        if admin {
            suffixes.extend(["/owners".into(), "/owner-allocations?metric=cpu".into()]);
        }
        for suffix in &suffixes {
            let path = format!("{prefix}/{id}{suffix}");
            let (status, positive) =
                request(&reopened, "GET", &path, "owner-1", admin, Value::Null).await;
            assert_eq!(status, 200, "owner {suffix}");
            if !admin {
                assert!(!positive.to_string().contains(foreign.sandbox_id.as_str()));
                assert!(!positive.to_string().contains(foreign.owner.as_str()));
            }
            let denied = request(&reopened, "GET", &path, "owner-2", admin, Value::Null).await;
            assert_eq!(denied.0, 404, "foreign {suffix}");
            assert_eq!(denied.1["code"], "not_found");
            let absent = request(
                &reopened,
                "GET",
                &format!("{prefix}/umr_00000000000000000000000000000000{suffix}"),
                "owner-2",
                admin,
                Value::Null,
            )
            .await;
            assert_eq!(denied, absent, "existence must remain concealed");
            if admin {
                let ordinary =
                    request(&reopened, "GET", &path, "owner-1", false, Value::Null).await;
                assert_eq!(ordinary.0, 403);
                assert_eq!(ordinary.1["code"], "forbidden");
            }
        }
        // Retained report rows cannot authorize a child belonging to a different
        // owner, even when the report ID itself belongs to the caller.
        if !admin {
            let (status, body) = request(
                &reopened,
                "GET",
                &format!("{prefix}/{id}/sandboxes/{}", foreign.sandbox_id),
                "owner-1",
                false,
                Value::Null,
            )
            .await;
            assert_eq!(status, 404);
            assert_eq!(body["code"], "not_found");
        }
        // Revocation represented by a changed authoritative parent invalidates
        // every cached projection, not only the report metadata endpoint.
        store
            .collection("sandboxes")
            .insert_one(doc! {
                "_id":own.sandbox_id.as_str(), "owner":"new-owner"
            })
            .await
            .unwrap();
        for suffix in &suffixes {
            let (status, body) = request(
                &reopened,
                "GET",
                &format!("{prefix}/{id}{suffix}"),
                "owner-1",
                admin,
                Value::Null,
            )
            .await;
            assert_eq!(status, 404, "stale report {suffix}");
            assert_eq!(body["code"], "not_found");
        }
        store
            .collection("sandboxes")
            .delete_one(doc! {"_id":own.sandbox_id.as_str()})
            .await
            .unwrap();
    }
    // Body-carried IDs receive the same ownership check before queue allocation.
    let before = store
        .collection(JOBS)
        .count_documents(doc! {})
        .await
        .unwrap();
    let (status, body) = request(
        &store,
        "POST",
        "/api/v1/usage/metrics/reports",
        "owner-1",
        false,
        json!({"from":from,"to":from+60,"filters":{"sandbox_id":foreign.sandbox_id}}),
    )
    .await;
    assert_eq!(status, 404);
    assert_eq!(body["code"], "not_found");
    assert_eq!(
        store
            .collection(JOBS)
            .count_documents(doc! {})
            .await
            .unwrap(),
        before
    );
    store.db.drop().await.unwrap();
}
