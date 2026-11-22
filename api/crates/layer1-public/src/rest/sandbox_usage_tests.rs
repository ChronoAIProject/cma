use super::*;
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use layer2_core::sandbox::store::Page;
use layer2_core::sandbox_usage_cursor::{UsageCursorPolicy, UsageCursors};
use layer2_core::sandbox_usage_service::{SandboxUsageStore, UsageSummary};
use layer2_core::{Authorization, Capability};
use std::sync::Mutex;
use tower::ServiceExt;

#[path = "sandbox_usage_tests/continuation.rs"]
mod continuation;

#[derive(Default)]
struct Store(
    Mutex<Vec<Option<SubjectId>>>,
    Mutex<Vec<PageRequest>>,
    Mutex<Vec<SandboxId>>,
);

#[test]
fn retained_terminal_rows_project_stopped_compute_without_discarding_disk_or_totals() {
    for status in ["failed", "retired"] {
        let state = SandboxUsage {
            sandbox_id: SandboxId("terminal-usage".into()),
            owner: SubjectId("owner".into()),
            name: "retained sandbox".into(),
            status: status.into(),
            allocation: Allocation {
                running: true,
                cpu_millis: 1_000,
                memory_mib: 2_048,
                disk_gib: 10,
            },
            created_at: 100,
            tracking_started_at: 100,
            last_observed_at: 160,
            source_seq: 2,
            source_revision: 3,
            revision: 2,
            has_started: true,
            complete_lifecycle: true,
            totals: UsageTotals {
                uptime_seconds: 60,
                cpu_millis_seconds: 60_000,
                memory_mib_seconds: 122_880,
                disk_gib_seconds: 600,
            },
        };
        let projected = SandboxUsageView::project(state, 220).unwrap();
        let body = serde_json::to_value(projected).unwrap();
        assert_eq!(body["status"], status);
        assert_eq!(
            body["allocation"],
            serde_json::json!({"running":false,"cpu_millis":0,"memory_mib":0,"disk_gib":10})
        );
        assert_eq!(body["totals"]["uptime_seconds"], 60);
        assert_eq!(body["totals"]["cpu_millis_seconds"], 60_000);
        assert_eq!(body["totals"]["memory_mib_seconds"], 122_880);
        assert_eq!(body["totals"]["disk_gib_seconds"], 1_200);
    }
}

pub(crate) fn router() -> Router {
    sandbox_usage_router(SandboxUsageService::new(
        Arc::new(Store::default()),
        UsageCursors::new(&[7; 32], UsageCursorPolicy::default()),
    ))
}
#[async_trait]
impl SandboxUsageStore for Store {
    async fn usage(&self, id: &SandboxId) -> Result<Option<SandboxUsage>, SandboxStoreError> {
        self.2.lock().unwrap().push(id.clone());
        Ok((id.as_str() == "known").then(|| SandboxUsage {
            sandbox_id: id.clone(),
            owner: SubjectId("owner".into()),
            name: "Usage fixture".into(),
            status: "paused".into(),
            allocation: Allocation {
                running: false,
                cpu_millis: 0,
                memory_mib: 0,
                disk_gib: 0,
            },
            created_at: 100,
            tracking_started_at: 100,
            last_observed_at: 100,
            source_seq: 1,
            source_revision: 1,
            revision: 1,
            has_started: false,
            complete_lifecycle: true,
            totals: UsageTotals::default(),
        }))
    }
    async fn usage_page(
        &self,
        owner: Option<&SubjectId>,
        page: &PageRequest,
    ) -> Result<Page<SandboxUsage>, SandboxStoreError> {
        self.0.lock().unwrap().push(owner.cloned());
        self.1.lock().unwrap().push(page.clone());
        Ok(Page {
            items: vec![],
            next: None,
        })
    }
    async fn usage_summary(
        &self,
        owner: Option<&SubjectId>,
        _: u64,
    ) -> Result<UsageSummary, SandboxStoreError> {
        self.0.lock().unwrap().push(owner.cloned());
        Ok(UsageSummary {
            sandbox_count: 2,
            running_count: 1,
            incomplete_lifecycle_count: 1,
            tracking_started_at: Some(100),
            totals: UsageTotals {
                uptime_seconds: 3600,
                cpu_millis_seconds: 7_200_000,
                memory_mib_seconds: 3_686_400,
                disk_gib_seconds: 36_000,
            },
        })
    }
    async fn usage_events(
        &self,
        id: &SandboxId,
        page: &PageRequest,
    ) -> Result<Page<UsageEvent>, SandboxStoreError> {
        assert_eq!(id.as_str(), "known");
        self.1.lock().unwrap().push(page.clone());
        Ok(Page {
            items: vec![],
            next: None,
        })
    }
}

async fn call(path: &str, capabilities: Option<&[Capability]>, store: Arc<Store>) -> Response {
    call_with_policy(path, capabilities, store, UsageCursorPolicy::Strict).await
}

async fn call_with_policy(
    path: &str,
    capabilities: Option<&[Capability]>,
    store: Arc<Store>,
    policy: UsageCursorPolicy,
) -> Response {
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    if let Some(capabilities) = capabilities {
        request
            .extensions_mut()
            .insert(crate::AuthenticatedSubject(SubjectId("owner".into())));
        request
            .extensions_mut()
            .insert(Authorization::new(capabilities.iter().copied()));
    }
    let response = sandbox_usage_router(SandboxUsageService::new(
        store,
        UsageCursors::new(&[7; 32], policy),
    ))
    .oneshot(request)
    .await
    .unwrap();
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    response
}

#[tokio::test]
async fn summary_uses_authenticated_owner_and_converts_integer_units() {
    let store = Arc::new(Store::default());
    let response = call(
        "/api/v1/usage",
        Some(&[Capability::UseOwnSandboxes]),
        store.clone(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(body["measurement"], "allocated_resource_time");
    assert_eq!(body["incomplete_lifecycle_count"], 1);
    assert_eq!(body["totals"]["cpu_core_hours"], 2.0);
    assert_eq!(body["totals"]["memory_gib_hours"], 1.0);
    assert_eq!(body["totals"]["disk_gib_hours"], 10.0);
    assert_eq!(
        *store.0.lock().unwrap(),
        vec![Some(SubjectId("owner".into()))]
    );
}

#[tokio::test]
async fn public_routes_refuse_missing_or_insufficient_authority_before_reads() {
    let store = Arc::new(Store::default());
    for path in [
        "/api/v1/usage",
        "/api/v1/usage/sandboxes",
        "/api/v1/usage/sandboxes/missing",
        "/api/v1/usage/sandboxes/missing/events",
        "/api/v1/admin/usage",
        "/api/v1/admin/usage/sandboxes",
    ] {
        assert_eq!(
            call(path, None, store.clone()).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for path in [
        "/api/v1/admin/usage",
        "/api/v1/admin/usage/sandboxes",
        "/api/v1/usage/sandboxes?owner=another-owner",
    ] {
        assert_eq!(
            call(path, Some(&[Capability::UseOwnSandboxes]), store.clone())
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert!(store.0.lock().unwrap().is_empty());
    assert_eq!(
        call(
            "/api/v1/usage/sandboxes/missing/events",
            Some(&[Capability::UseOwnSandboxes]),
            store.clone()
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn admin_filter_and_pagination_are_validated_at_the_http_boundary() {
    let store = Arc::new(Store::default());
    let admin = Some(&[Capability::ReadAllSandboxes][..]);
    for path in [
        "/api/v1/admin/usage",
        "/api/v1/admin/usage/sandboxes?owner=another-owner&limit=1",
    ] {
        assert_eq!(
            call(path, admin, store.clone()).await.status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        *store.0.lock().unwrap(),
        vec![None, Some(SubjectId("another-owner".into()))]
    );
    for query in ["limit=0", "limit=201", "limit=invalid", "unknown=value"] {
        assert_eq!(
            call(
                &format!("/api/v1/admin/usage/sandboxes?{query}"),
                admin,
                store.clone()
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(store.0.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn fabricated_usage_continuations_are_typed_refusals_before_list_reads() {
    let store = Arc::new(Store::default());
    for cursor in ["invalid", "zzz"] {
        let response = call(
            &format!("/api/v1/usage/sandboxes?cursor={cursor}"),
            Some(&[Capability::UseOwnSandboxes]),
            store.clone(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(body["code"], "invalid_query");
    }
    assert!(store.0.lock().unwrap().is_empty());
}

async fn invalid_query(response: Response) {
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 16384).await.unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "type": "https://spec.chrono-ai.fun/cma/1.0/problems/invalid_query",
            "title": "invalid_query",
            "status": 400,
            "detail": "Use a valid cursor and a limit from 1 through 200.",
            "code": "invalid_query"
        })
    );
}

#[tokio::test]
async fn pagination_query_refusals_are_fixed_problems_before_store_reads() {
    let store = Arc::new(Store::default());
    for path in [
        "/api/v1/usage/sandboxes",
        "/api/v1/admin/usage/sandboxes",
        "/api/v1/usage/sandboxes/known/events",
    ] {
        for query in [
            "limit=-1",
            "limit=0",
            "limit=201",
            "limit=65536",
            "limit=abc",
            "unknown=private-input",
        ] {
            invalid_query(
                call(
                    &format!("{path}?{query}"),
                    Some(&[Capability::UseOwnSandboxes, Capability::ReadAllSandboxes]),
                    store.clone(),
                )
                .await,
            )
            .await;
        }
    }
    assert!(store.0.lock().unwrap().is_empty());
    assert!(store.1.lock().unwrap().is_empty());
    assert!(store.2.lock().unwrap().is_empty());
}

#[tokio::test]
async fn usage_pagination_preserves_boundaries_default_and_cursor() {
    for (path, cursor) in [
        ("/api/v1/usage/sandboxes", "retained-cursor"),
        ("/api/v1/admin/usage/sandboxes", "retained-cursor"),
        ("/api/v1/usage/sandboxes/known/events", "17"),
    ] {
        let store = Arc::new(Store::default());
        for (query, limit) in [("", 50), ("limit=1&", 1), ("limit=200&", 200)] {
            let response = call_with_policy(
                &format!("{path}?{query}cursor={cursor}"),
                Some(&[Capability::UseOwnSandboxes, Capability::ReadAllSandboxes]),
                store.clone(),
                UsageCursorPolicy::ReaderOnly,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                store.1.lock().unwrap().last(),
                Some(&PageRequest {
                    limit,
                    cursor: Some(cursor.into()),
                })
            );
        }
    }
}

#[tokio::test]
async fn admin_summary_refuses_malformed_queries_without_adding_pagination() {
    let store = Arc::new(Store::default());
    let admin = Some(&[Capability::ReadAllSandboxes][..]);
    for query in [
        "limit=-1",
        "limit=65536",
        "limit=abc",
        "unknown=private-input",
    ] {
        invalid_query(
            call(
                &format!("/api/v1/admin/usage?{query}"),
                admin,
                store.clone(),
            )
            .await,
        )
        .await;
    }
    assert!(store.0.lock().unwrap().is_empty());
    for limit in [0, 201, 65535] {
        let response = call(
            &format!("/api/v1/admin/usage?limit={limit}&owner=another-owner"),
            admin,
            store.clone(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    assert_eq!(
        *store.0.lock().unwrap(),
        vec![Some(SubjectId("another-owner".into())); 3]
    );
    assert!(store.1.lock().unwrap().is_empty());
}

#[tokio::test]
async fn malformed_usage_queries_preserve_authentication_precedence() {
    let store = Arc::new(Store::default());
    for path in [
        "/api/v1/usage/sandboxes",
        "/api/v1/admin/usage/sandboxes",
        "/api/v1/usage/sandboxes/known/events",
        "/api/v1/admin/usage",
    ] {
        let response = call(&format!("{path}?limit=-1"), None, store.clone()).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
    }
    assert!(store.0.lock().unwrap().is_empty());
    assert!(store.1.lock().unwrap().is_empty());
    assert!(store.2.lock().unwrap().is_empty());
}
