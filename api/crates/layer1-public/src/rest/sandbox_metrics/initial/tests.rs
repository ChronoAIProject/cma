use super::*;
mod contracts;
mod store;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use layer2_core::{Authorization, Capability, SubjectId};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use store::Store;
use tower::ServiceExt;
const REPORT: &str = "umr_00000000000000000000000000000001";
const OWNER: &[Capability] = &[Capability::UseOwnSandboxes];
const ADMIN: &[Capability] = &[Capability::ReadAllSandboxes];

async fn call(
    store: &Arc<Store>,
    path: &str,
    who: Option<(&str, &[Capability])>,
    body: Value,
) -> Response {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    if let Some((subject, caps)) = who {
        request
            .extensions_mut()
            .insert(crate::AuthenticatedSubject(SubjectId(subject.into())));
        request
            .extensions_mut()
            .insert(Authorization::new(caps.iter().copied()));
    }
    let response = sandbox_metrics_router(MetricsService::new(store.clone()))
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    response
}
async fn body(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), encoding::MAX_INITIAL_BYTES)
            .await
            .unwrap(),
    )
    .unwrap()
}
fn projection() -> Value {
    json!({"total_metrics":["cpu","memory","disk"]})
}
fn request(suffix: &str) -> Value {
    if suffix == "/view" {
        json!({"selection":{},"projection":projection()})
    } else if suffix == "/reports" {
        json!({"query":{},"projection":projection()})
    } else {
        projection()
    }
}

#[tokio::test]
async fn initial_routes_deliver_complete_owner_admin_and_visitor_views_with_one_authorization() {
    for (prefix, capability) in [
        ("/api/v1/usage/metrics", OWNER),
        ("/api/v1/usage/metrics", &[Capability::LaunchTriggers][..]),
        ("/api/v1/admin/usage/metrics", ADMIN),
    ] {
        for suffix in ["/reports", "/view"] {
            for members in [0, 1] {
                let store = Store::new("", members);
                let response = call(
                    &store,
                    &format!("{prefix}/initial{suffix}"),
                    Some(("owner-a", capability)),
                    request(suffix),
                )
                .await;
                assert_eq!(response.status(), 200);
                assert!(response.headers().get(header::RETRY_AFTER).is_none());
                let result = body(response).await;
                let report = if suffix == "/view" {
                    &result["view"]["report"]
                } else {
                    &result["report"]
                };
                assert_eq!(report["status"], "ready");
                assert_eq!(
                    result["initial"]["rows"]["items"].as_array().unwrap().len() as u64,
                    members
                );
                assert_eq!(result["initial"]["charts"].as_array().unwrap().len(), 3);
                assert_eq!(store.auth.load(Ordering::SeqCst), 1);
                assert_eq!(store.pages.load(Ordering::SeqCst), 1);
                assert_eq!(store.charts.load(Ordering::SeqCst), 3);
                let response = call(
                    &store,
                    &format!("{prefix}/initial/reports/{REPORT}"),
                    Some(("owner-a", capability)),
                    json!({}),
                )
                .await;
                assert_eq!(response.status(), 200);
                assert_eq!(store.admissions.load(Ordering::SeqCst), 1);
                assert_eq!(store.auth.load(Ordering::SeqCst), 2);
                assert_eq!(store.charts.load(Ordering::SeqCst), 3);
                store.base.revoked.store(true, Ordering::SeqCst);
                let response = call(
                    &store,
                    &format!("{prefix}/initial/reports/{REPORT}"),
                    Some(("owner-a", capability)),
                    projection(),
                )
                .await;
                assert_eq!(response.status(), 404);
                assert_eq!(store.pages.load(Ordering::SeqCst), 2);
            }
        }
    }
}

#[tokio::test]
async fn initial_pending_keeps_id_without_children_or_readmission() {
    for suffix in ["/reports", "/view"] {
        let store = Store::new("pending", 1);
        let prefix = "/api/v1/usage/metrics/initial";
        let response = call(
            &store,
            &format!("{prefix}{suffix}"),
            Some(("owner-a", OWNER)),
            request(suffix),
        )
        .await;
        assert_eq!(response.status(), 202);
        assert_eq!(response.headers()[header::RETRY_AFTER], "2");
        let result = body(response).await;
        assert!(result.get("initial").is_none());
        let report = if suffix == "/view" {
            &result["view"]["report"]
        } else {
            &result["report"]
        };
        assert_eq!(report["report_id"], REPORT);
        for _ in 0..2 {
            let response = call(
                &store,
                &format!("{prefix}/reports/{REPORT}"),
                Some(("owner-a", OWNER)),
                projection(),
            )
            .await;
            assert_eq!(response.status(), 202);
            let result = body(response).await;
            assert!(result.get("initial").is_none());
            assert_eq!(result["report"]["report_id"], REPORT);
        }
        assert_eq!(store.admissions.load(Ordering::SeqCst), 1);
        assert_eq!(store.pages.load(Ordering::SeqCst), 0);
        assert_eq!(store.charts.load(Ordering::SeqCst), 0);
        assert_eq!(store.auth.load(Ordering::SeqCst), 3);
    }
}

#[tokio::test(start_paused = true)]
async fn initial_child_failure_size_overflow_and_deadline_never_return_partial_data() {
    for fault in ["child-error", "oversize", "slow"] {
        let store = Store::new(fault, 1);
        let response = call(
            &store,
            "/api/v1/usage/metrics/initial/reports",
            Some(("owner-a", OWNER)),
            request("/reports"),
        )
        .await;
        assert_eq!(response.status(), 503, "{fault}");
        assert_eq!(response.headers()[header::RETRY_AFTER], "2");
        let result = body(response).await;
        assert_eq!(result["code"], "usage_unavailable");
        assert!(result.get("initial").is_none());
        assert!(result.get("report").is_none());
    }
}

#[tokio::test]
async fn initial_latest_preserves_staleness_and_typed_unsupported() {
    for (fault, expected) in [("stale", 200), ("disabled", 501)] {
        let store = Store::new(fault, 1);
        let response = call(
            &store,
            "/api/v1/admin/usage/metrics/initial/view",
            Some(("owner-a", ADMIN)),
            request("/view"),
        )
        .await;
        assert_eq!(response.status(), expected);
        assert_eq!(
            response
                .extensions()
                .get::<crate::rest::UsageReportViewDisabled>()
                .is_some(),
            expected == 501
        );
        let result = body(response).await;
        if expected == 200 {
            assert_eq!(result["view"]["stale"], true);
            assert!(result["initial"].is_object());
        } else {
            assert_eq!(result["code"], "report_view_disabled");
            assert_eq!(store.auth.load(Ordering::SeqCst), 0);
        }
    }
}
