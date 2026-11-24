use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use layer2_core::sandbox_metrics::OwnerContextAccess;
use layer2_core::{Authorization, Capability};
use serde_json::Value;
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_owner_labels_distinguish_page_boundaries_with_authorized_context() {
    let store = fixture().await;
    let at = now();
    let report = store
        .submit(
            scope("operator", true),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "boundary")
        .await
        .unwrap()
        .unwrap();
    let states: Vec<_> = (0..201)
        .map(|n| {
            let mut state = state(n, at - 3600);
            state.owner = SubjectId(format!("owner-{n:06}"));
            state
        })
        .collect();
    seed(&store, &states).await;
    let rows: Vec<_> = states
        .iter()
        .map(|state| {
            let mut row = fixture_row(state);
            row.allocated_period.cpu_core_hours = 1.;
            row
        })
        .collect();
    store
        .write_report_chunk(&lease, &rows, vec![])
        .await
        .unwrap();
    store
        .finish_report(&lease, report.clone(), true)
        .await
        .unwrap();
    store
        .collection("users")
        .insert_many((0..201).map(|n| {
            doc! {
                "_id":format!("owner-{n:06}"),
                "display_name":if n >= 199 {"Alex Chen".into()} else {format!("Owner {n}")}
            }
        }))
        .await
        .unwrap();

    for (timestamps, access, reference) in [
        (
            ["2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z"],
            OwnerContextAccess::DateOnly,
            false,
        ),
        (
            ["2026-01-01T10:00:00Z", "2026-01-01T11:00:00Z"],
            OwnerContextAccess::DirectoryRead,
            false,
        ),
        (
            ["2026-01-01T10:00:00Z", "2026-01-01T11:00:00Z"],
            OwnerContextAccess::DateOnly,
            true,
        ),
        (
            ["2026-01-01T10:00:00Z", "2026-01-01T10:00:00Z"],
            OwnerContextAccess::DirectoryRead,
            true,
        ),
        (["invalid", "invalid"], OwnerContextAccess::DateOnly, true),
    ] {
        for (index, timestamp) in timestamps.iter().enumerate() {
            store
                .collection("users")
                .update_one(
                    doc! {"_id":format!("owner-{:06}",199+index)},
                    doc! {"$set":{"first_seen_at":timestamp}},
                )
                .await
                .unwrap();
        }
        let query = |limit, cursor| OwnerAllocationQuery {
            metric: Metric::Cpu,
            limit: Some(limit),
            cursor,
        };
        let first = store
            .owner_allocations(&report, query(200, None), access)
            .await
            .unwrap();
        let second = store
            .owner_allocations(&report, query(200, first.next.clone()), access)
            .await
            .unwrap();
        assert!(first.items[..199]
            .iter()
            .all(|item| item.disambiguator.is_none()));
        assert!(second.next.is_none());
        let pair = [&first.items[199], &second.items[0]];
        assert_eq!(pair[0].label, "Alex Chen");
        assert_eq!(pair[1].label, "Alex Chen");
        assert_ne!(pair[0].disambiguator, pair[1].disambiguator);
        for item in pair {
            let detail = item.disambiguator.as_ref().unwrap();
            assert_eq!(detail.contains("reference"), reference, "{detail}");
            assert!(!detail.contains(&item.subject));
            if matches!(access, OwnerContextAccess::DateOnly) {
                assert!(!detail.contains("UTC"));
            }
        }
        // Identity is independent of page size and of whether a peer is on this page.
        let tiny = store
            .owner_allocations(&report, query(1, first.next), access)
            .await
            .unwrap();
        assert_eq!(tiny.items[0].disambiguator, pair[1].disambiguator);
    }
    store.db.drop().await.unwrap();
}

async fn read_allocation_page(
    router: &Router,
    path: &str,
    capabilities: &[Capability],
) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .extension(layer1_public::AuthenticatedSubject(SubjectId::new(
                    "operator",
                )))
                .extension(Authorization::new(capabilities.iter().copied()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn metrics_owner_allocation_http_cursor_uses_current_directory_authority() {
    let store = fixture().await;
    let at = now();
    let report = store
        .submit(
            scope("operator", true),
            ReportInput::default().validate(at).unwrap(),
            at,
        )
        .await
        .unwrap();
    let lease = store
        .claim_report("report:0", "allocation-authority")
        .await
        .unwrap()
        .unwrap();
    let owners = [
        ("owner-first", "2026-01-01T10:12:00Z"),
        ("owner-second", "2026-01-01T11:37:00Z"),
    ];
    let states: Vec<_> = owners
        .iter()
        .enumerate()
        .map(|(index, (owner, _))| {
            let mut state = state(index, at - 3600);
            state.owner = SubjectId::new(*owner);
            state
        })
        .collect();
    seed(&store, &states).await;
    let rows: Vec<_> = states
        .iter()
        .map(|state| {
            let mut row = fixture_row(state);
            row.allocated_period.cpu_core_hours = 2.5;
            row.allocated_period.complete = true;
            row
        })
        .collect();
    store
        .write_report_chunk(&lease, &rows, vec![])
        .await
        .unwrap();
    store
        .finish_report(&lease, report.clone(), true)
        .await
        .unwrap();
    store
        .collection("users")
        .insert_many(owners.map(|(owner, timestamp)| {
            doc! {"_id":owner,"display_name":"Alex Chen","first_seen_at":timestamp}
        }))
        .await
        .unwrap();

    let router = layer1_public::rest::sandbox_metrics_router(MetricsService::new(store.clone()));
    let path = format!(
        "/api/v1/admin/usage/metrics/reports/{}/owner-allocations?metric=cpu&limit=1",
        report.report_id
    );
    let directory = [Capability::ReadAllSandboxes, Capability::ReadUserDirectory];
    let (status, first) = read_allocation_page(&router, &path, &directory).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["report_id"], report.report_id);
    assert_eq!(first["metric"], "cpu");
    assert_eq!(first["total_count"], 2);
    assert_eq!(
        first["items"],
        serde_json::json!([{
            "subject":"owner-first",
            "label":"Alex Chen",
            "disambiguator":"Joined 2026-01-01 10:12 UTC",
            "allocated_hours":2.5,
            "sandbox_count":1,
            "complete":true,
        }])
    );
    let cursor = first["next"].as_str().unwrap();
    let cursor_query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("cursor", cursor)
        .finish();
    let continuation = format!("{path}&{cursor_query}");
    let (status, mut full) = read_allocation_page(&router, &continuation, &directory).await;
    assert_eq!(status, StatusCode::OK, "{full}");
    assert_eq!(full["report_id"], report.report_id);
    assert_eq!(full["metric"], first["metric"]);
    assert_eq!(full["unit"], first["unit"]);
    assert_eq!(full["total_count"], first["total_count"]);
    assert!(full["next"].is_null());
    assert_eq!(
        full["items"],
        serde_json::json!([{
            "subject":"owner-second",
            "label":"Alex Chen",
            "disambiguator":"Joined 2026-01-01 11:37 UTC",
            "allocated_hours":2.5,
            "sandbox_count":1,
            "complete":true,
        }])
    );

    let (status, restricted) =
        read_allocation_page(&router, &continuation, &[Capability::ReadAllSandboxes]).await;
    assert_eq!(status, StatusCode::OK, "{restricted}");
    let detail = restricted["items"][0]["disambiguator"].as_str().unwrap();
    let reference = detail
        .strip_prefix("Joined 2026-01-01 · reference ")
        .unwrap();
    assert_eq!(reference.len(), 12);
    assert!(reference.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(!restricted.to_string().contains("11:37"));
    full["items"][0]["disambiguator"] = Value::String(detail.into());
    assert_eq!(restricted, full);

    let (status, denied) = read_allocation_page(
        &router,
        &continuation,
        &[Capability::UseOwnSandboxes, Capability::ReadUserDirectory],
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{denied}");
    assert_eq!(denied["code"], "forbidden");
    assert!(denied.get("items").is_none());
    store.db.drop().await.unwrap();
}
