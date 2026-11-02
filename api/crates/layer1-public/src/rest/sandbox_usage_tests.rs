use super::*;
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use layer2_core::sandbox::store::Page;
use layer2_core::sandbox_usage_service::{SandboxUsageStore, UsageSummary};
use layer2_core::{Authorization, Capability};
use std::sync::Mutex;
use tower::ServiceExt;

#[derive(Default)]
struct Store(Mutex<Vec<Option<SubjectId>>>);

pub(crate) fn router() -> Router {
    sandbox_usage_router(SandboxUsageService::new(Arc::new(Store::default())))
}
#[async_trait]
impl SandboxUsageStore for Store {
    async fn usage(&self, _: &SandboxId) -> Result<Option<SandboxUsage>, SandboxStoreError> {
        Ok(None)
    }
    async fn usage_page(
        &self,
        owner: Option<&SubjectId>,
        _: &PageRequest,
    ) -> Result<Page<SandboxUsage>, SandboxStoreError> {
        self.0.lock().unwrap().push(owner.cloned());
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
        _: &SandboxId,
        _: &PageRequest,
    ) -> Result<Page<UsageEvent>, SandboxStoreError> {
        panic!("history must not be read for an unknown Sandbox")
    }
}

async fn call(path: &str, capabilities: Option<&[Capability]>, store: Arc<Store>) -> Response {
    let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
    if let Some(capabilities) = capabilities {
        request
            .extensions_mut()
            .insert(crate::AuthenticatedSubject(SubjectId("owner".into())));
        request
            .extensions_mut()
            .insert(Authorization::new(capabilities.iter().copied()));
    }
    let response = sandbox_usage_router(SandboxUsageService::new(store))
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
