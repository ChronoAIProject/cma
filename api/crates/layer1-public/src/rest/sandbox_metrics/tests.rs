use super::*;
use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use layer2_core::{Authorization, Capability, SubjectId};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tower::ServiceExt;
const REPORT: &str = "umr_00000000000000000000000000000001";
const SANDBOX: &str = "sbx_00000000000000000000000000000001";
#[derive(Default)]
pub(super) struct Store {
    pub(super) saved: Mutex<Option<Report>>,
    pub(super) revoked: AtomicBool,
}
#[async_trait]
impl MetricsStore for Store {
    async fn owner_allocations(
        &self,
        r: &Report,
        q: OwnerAllocationQuery,
    ) -> Result<OwnerAllocations> {
        Ok(OwnerAllocations {
            report_id: r.report_id.clone(),
            metric: q.metric,
            unit: q.unit().into(),
            items: vec![],
            next: None,
            total_count: 0,
        })
    }
    async fn options(&self, scope: &Scope, query: OptionQuery) -> Result<FilterOptions> {
        assert!(scope.admin);
        assert!(
            self.saved.lock().unwrap().is_none(),
            "discovery must not create a report"
        );
        if self.revoked.load(Ordering::SeqCst) {
            return Err(MetricsError::Unavailable);
        }
        Ok(FilterOptions {
            items: vec![FilterOption {
                value: "owner-a".into(),
                label: option_label(query.kind, "owner-a", Some("Alex")),
                disambiguator: None,
            }],
            next: Some("next-page".into()),
        })
    }
    async fn resolve_agent(&self, scope: &Scope, _: &str) -> Result<String> {
        self.authorize_sandbox(scope, SANDBOX).await?;
        Ok(SANDBOX.into())
    }
    async fn authorize_sandbox(&self, scope: &Scope, id: &str) -> Result<()> {
        if id != SANDBOX
            || (!scope.admin && scope.principal != "owner-a")
            || self.revoked.load(Ordering::SeqCst)
        {
            Err(MetricsError::NotFound)
        } else {
            Ok(())
        }
    }
    async fn submit(&self, scope: Scope, query: ReportQuery, at: u64) -> Result<Report> {
        if let Some(report) = self.saved.lock().unwrap().clone() {
            return Ok(report);
        }
        let report = Report {
            schema_version: 1,
            report_id: REPORT.into(),
            status: ReportStatus::Ready,
            report_complete: true,
            effective_range: query.effective_range(at),
            scope,
            query,
            as_of: at,
            measurement_cutoff: at - 120,
            expires_at: at + 600,
            allocation_watermark: "fixture".into(),
            mapping_revision: 1,
            available_from: None,
            tracking_started_at: None,
            warnings: vec![],
            source_health: "source_unavailable".into(),
            summary: Some(Summary::default()),
            poll_url: format!("/api/v1/usage/metrics/reports/{REPORT}"),
            retry_after_seconds: 2,
        };
        *self.saved.lock().unwrap() = Some(report.clone());
        Ok(report)
    }
    async fn report(&self, scope: &Scope, _: &str, _: u64) -> Result<Report> {
        self.saved
            .lock()
            .unwrap()
            .clone()
            .filter(|r| &r.scope == scope)
            .ok_or(MetricsError::NotFound)
    }
    async fn reauthorize(&self, _: &Report) -> Result<()> {
        if self.revoked.load(Ordering::SeqCst) {
            Err(MetricsError::NotFound)
        } else {
            Ok(())
        }
    }
    async fn page(&self, r: &Report, _: PageQuery) -> Result<Rows> {
        Ok(Rows {
            items: vec![],
            next: None,
            report_id: r.report_id.clone(),
            as_of: r.as_of,
            total_count: 0,
        })
    }
    async fn detail(&self, _: &Report, _: &str) -> Result<Row> {
        Err(MetricsError::NotFound)
    }
    async fn series(&self, r: &Report, q: SeriesQuery) -> Result<Series> {
        Ok(Series {
            report_id: r.report_id.clone(),
            metric: q.metric,
            unit: q.metric.unit().into(),
            source: SOURCE.into(),
            lines: vec![],
        })
    }
}
pub(crate) fn router() -> Router {
    sandbox_metrics_router(MetricsService::new(Arc::new(Store::default())))
}
pub(super) async fn call(
    store: &Arc<Store>,
    method: &str,
    path: &str,
    subject: Option<(&str, &[Capability])>,
    body: &str,
) -> Response {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap();
    if let Some((owner, caps)) = subject {
        req.extensions_mut()
            .insert(crate::AuthenticatedSubject(SubjectId(owner.into())));
        req.extensions_mut()
            .insert(Authorization::new(caps.iter().copied()));
    }
    let response = sandbox_metrics_router(MetricsService::new(store.clone()))
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-store"
    );
    response
}
#[tokio::test]
async fn all_routes_recheck_capabilities_and_cached_owner_authority() {
    let store = Arc::new(Store::default());
    let owner = Some(("owner-a", &[Capability::UseOwnSandboxes][..]));
    let base = "/api/v1/usage/metrics/reports";
    assert_eq!(
        call(&store, "POST", base, owner, "{}").await.status(),
        StatusCode::OK
    );
    for suffix in [
        "".to_owned(),
        "/sandboxes".into(),
        "/series?metric=cpu".into(),
        format!("/sandboxes/{SANDBOX}"),
    ] {
        let path = format!("{base}/{REPORT}{suffix}");
        assert_eq!(
            call(&store, "GET", &path, None, "").await.status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            call(&store, "GET", &path, Some(("owner-a", &[])), "")
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            call(
                &store,
                "GET",
                &path,
                Some(("owner-b", &[Capability::UseOwnSandboxes])),
                ""
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        let admin = path.replace("/api/v1/usage", "/api/v1/admin/usage");
        assert_eq!(
            call(&store, "GET", &admin, owner, "").await.status(),
            StatusCode::FORBIDDEN
        );
    }
    store.revoked.store(true, Ordering::SeqCst);
    for path in [
        format!("{base}/{REPORT}"),
        format!("{base}/{REPORT}/sandboxes"),
        format!("{base}/{REPORT}/series?metric=disk"),
    ] {
        assert_eq!(
            call(&store, "GET", &path, owner, "").await.status(),
            StatusCode::NOT_FOUND
        );
    }
    // Coalescing must perform the same revocation check as subsequent reads.
    assert_eq!(
        call(&store, "POST", base, owner, "{}").await.status(),
        StatusCode::NOT_FOUND
    );
}
#[tokio::test]
async fn typed_queries_reject_source_passthrough_and_owner_forgery() {
    let store = Arc::new(Store::default());
    let owner = Some(("owner-a", &[Capability::LaunchTriggers][..]));
    let base = "/api/v1/usage/metrics/reports";
    for input in [
        r#"{"query":"up"}"#,
        r#"{"filters":{"owner":"owner-b"}}"#,
        r#"{"filters":{"statuses":["invented"]}}"#,
        r#"{"from":100,"to":99}"#,
        r#"{"step_seconds":1}"#,
        r#"{"filters":{"sandbox_id":".*"}}"#,
    ] {
        assert_eq!(
            call(&store, "POST", base, owner, input).await.status(),
            StatusCode::BAD_REQUEST,
            "{input}"
        );
    }
    let response = call(&store, "POST", base, owner, "{}").await;
    let body: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(body["scope"]["principal"], "owner-a");
    for suffix in [
        "?query=up",
        "/sandboxes?limit=201",
        "/sandboxes?sort=promql",
        "/series?metric=up",
        "/series?metric=cpu&top=21",
        "/series?metric=cpu&group_by=sandbox&series_id=arbitrary",
    ] {
        assert_eq!(
            call(
                &store,
                "GET",
                &format!("{base}/{REPORT}{suffix}"),
                owner,
                ""
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    for suffix in ["/sandboxes?sort=owner", "/series?metric=cpu&group_by=owner"] {
        assert_eq!(
            call(
                &store,
                "GET",
                &format!("{base}/{REPORT}{suffix}"),
                owner,
                ""
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
    }
}
#[tokio::test]
async fn expired_report_returns_gone_before_any_cached_projection() {
    let store = Arc::new(Store::default());
    let owner = Some(("owner-a", &[Capability::UseOwnSandboxes][..]));
    let base = "/api/v1/usage/metrics/reports";
    call(&store, "POST", base, owner, "{}").await;
    store.saved.lock().unwrap().as_mut().unwrap().expires_at = 0;
    assert_eq!(
        call(&store, "GET", &format!("{base}/{REPORT}"), owner, "")
            .await
            .status(),
        StatusCode::GONE
    );
}
