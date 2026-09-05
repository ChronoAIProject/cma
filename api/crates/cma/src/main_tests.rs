use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use layer1_public::{
    rest::{hierarchy_router, profiles_router, HierarchyState, ProfilesState},
    AuthenticatedSubject,
};
use layer2_core::{
    profiles::InMemoryProfileStore,
    sandbox::{
        HierarchyReadService, InMemorySandboxStore, SandboxPolicy, SandboxService, WorkspaceService,
    },
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Authorization, OperationClaim,
    OperationReservation, OperationStore, Page, PlatformSettings, PlatformSettingsService,
    PlatformSettingsStore, StoreError, StoredPlatformSettings, SubjectId,
};
use tower::ServiceExt as _;

use super::*;

#[derive(Default)]
struct NoopOperationStore;

#[async_trait]
impl OperationStore for NoopOperationStore {
    async fn reserve(&self, _claim: &OperationClaim) -> Result<OperationReservation, StoreError> {
        Ok(OperationReservation::Acquired)
    }

    async fn complete(&self, _claim: &OperationClaim, _outcome: Vec<u8>) -> Result<(), StoreError> {
        Ok(())
    }
}

#[derive(Default)]
struct NoopActivityStore;

#[async_trait]
impl ActivityStore for NoopActivityStore {
    async fn append(&self, _record: &ActivityRecord) -> Result<(), StoreError> {
        Ok(())
    }

    async fn list(&self, _query: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        Ok(Page::new(Vec::new(), None))
    }
}

struct MutableSettings(Mutex<StoredPlatformSettings>);

impl MutableSettings {
    fn new() -> Self {
        Self(Mutex::new(StoredPlatformSettings {
            revision: 0,
            settings: PlatformSettings::default(),
        }))
    }
}

#[async_trait]
impl PlatformSettingsStore for MutableSettings {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn replace(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError> {
        let mut stored = self.0.lock().unwrap();
        if stored.revision != expected_revision {
            return Err(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: stored.revision,
            });
        }
        *stored = StoredPlatformSettings {
            revision: expected_revision + 1,
            settings: settings.clone(),
        };
        Ok(stored.clone())
    }
}

fn composed_rest_router() -> Router {
    let profile_store = Arc::new(InMemoryProfileStore::default());
    let sandbox_store = Arc::new(InMemorySandboxStore::default());
    let operations: Arc<dyn OperationStore> = Arc::new(NoopOperationStore);
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::new(MutableSettings::new()),
        ActivityService::new(Arc::new(NoopActivityStore)),
    ));
    let state = ProfilesState::new(
        Arc::clone(&profile_store),
        Arc::clone(&operations),
        Arc::clone(&settings),
        profile_route_policy(),
    );
    let sandboxes = SandboxService::new(
        Arc::clone(&sandbox_store),
        Arc::clone(&profile_store),
        SandboxPolicy::default(),
    );
    let workspaces = WorkspaceService::new(Arc::clone(&sandbox_store), profile_store.clone());
    let projections = AgentProjectionServices::new(
        Arc::clone(&sandbox_store),
        profile_store.clone(),
        settings,
        Arc::clone(&sandboxes),
        Arc::clone(&workspaces),
        Arc::new(LiveAgentRegistry::default()),
        Arc::new(UnavailableSandboxResources),
        runtime_capabilities(layer3_adapter::RuntimeContract::V3.capabilities()),
    );
    let hierarchy = HierarchyState::new(
        sandboxes,
        workspaces,
        HierarchyReadService::new(sandbox_store),
        Arc::clone(&operations),
    );
    rest_router(
        profiles_router(state)
            .merge(hierarchy_router(hierarchy))
            .merge(projections.router(operations)),
    )
}

#[tokio::test]
async fn cors_preflight_grants_only_configured_origins_and_headers() {
    let app = with_cors(
        Router::new(),
        vec![HeaderValue::from_static("https://console.example")],
    );
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/api/v1/conformance")
                .header(header::ORIGIN, "https://console.example")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
        Some(&HeaderValue::from_static("https://console.example"))
    );
}

#[tokio::test]
async fn profile_routes_are_composed_before_the_rest_fallback() {
    let mut request = Request::get("/api/v1/agent-profiles")
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId::new("owner-a")));
    request.extensions_mut().insert(Authorization::none());
    let response = composed_rest_router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    assert_eq!(response.headers().get(header::LINK), Some(&REST_LINKS));
}

#[tokio::test]
async fn hierarchy_routes_are_composed_before_the_rest_fallback() {
    let mut request = Request::get("/api/v1/sandboxes")
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(AuthenticatedSubject(SubjectId::new("owner-a")));
    request.extensions_mut().insert(Authorization::none());
    let response = composed_rest_router().oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    assert_eq!(response.headers().get(header::LINK), Some(&REST_LINKS));
}

#[tokio::test]
async fn every_agent_interface_is_composed_before_the_rest_fallback() {
    let app = composed_rest_router();
    for (method, path, expected) in [
        (Method::GET, "/api/v1/agents", StatusCode::FORBIDDEN),
        (Method::GET, "/api/v1/agui/capabilities", StatusCode::OK),
        (
            Method::GET,
            "/api/v1/agents/agt_missing/terminal",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (Method::POST, "/api/v1/mcp", StatusCode::NOT_IMPLEMENTED),
        (Method::POST, "/api/v1/a2a", StatusCode::NOT_IMPLEMENTED),
        (
            Method::GET,
            "/.well-known/agent-card.json",
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            Method::GET,
            "/api/v1/responses",
            StatusCode::METHOD_NOT_ALLOWED,
        ),
    ] {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(AuthenticatedSubject(SubjectId::new("owner-a")));
        request.extensions_mut().insert(Authorization::none());
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected, "unexpected status for {path}");
        assert_eq!(response.headers().get(header::LINK), Some(&REST_LINKS));
    }
}

#[tokio::test]
async fn retired_session_and_terminal_ticket_routes_remain_absent() {
    let app = composed_rest_router();
    for path in [
        "/api/v1/sessions",
        "/api/v1/sessions/sesn_retired/terminal",
        "/api/v1/agents/agt_missing/terminal-ticket",
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(response.headers().get(header::LINK), Some(&REST_LINKS));
    }
}

#[tokio::test]
async fn unknown_rest_paths_keep_problem_and_profile_headers() {
    let response = composed_rest_router()
        .oneshot(Request::get("/api/v1/missing").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    assert_eq!(response.headers().get(header::LINK), Some(&REST_LINKS));
}
