use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use layer1_public::{
    agui::{launch_companion_router, LaunchCompanionState},
    rest::{
        hierarchy_router, profiles_router, trigger_assets_router, trigger_router, HierarchyState,
        LaunchQuota, LaunchQuotaError, LaunchQuotaPermit, ProfilesState, TriggerAssetsState,
        TriggerRoutesState,
    },
    AuthenticatedSubject,
};
use layer2_core::{
    profiles::InMemoryProfileStore,
    sandbox::{
        HierarchyReadService, InMemorySandboxStore, SandboxPolicy, SandboxService, WorkspaceService,
    },
    trigger_domain::{
        BrokerHandoff, BrokerHandoffState, InMemoryLaunchStore, InMemoryTriggerStore, LaunchPolicy,
        LaunchResolutionError, LaunchService, ServiceConnectionBroker, ServiceConnectionError,
        ServiceConnectionService, ServiceRequirement, TriggerId, TriggerService,
    },
    ActivityQuery, ActivityRecord, ActivityService, ActivityStore, Authorization, DirectoryQuery,
    OperationClaim, OperationReservation, OperationStore, Page, PlatformSettings,
    PlatformSettingsService, PlatformSettingsStore, PublicIdentity, RegisteredUser, SandboxId,
    SandboxProfileId, ServiceId, StoreError, StoredPlatformSettings, SubjectId, UserDirectory,
    UserDirectoryStore, WorkspaceProfileId,
};
use tower::ServiceExt as _;

use super::*;
use crate::{
    profile_policy::profile_route_policy,
    projections::{AgentProjectionServices, UnavailableSandboxResources},
    sandbox::{runtime_capabilities, LiveAgentRegistry},
};

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

#[derive(Default)]
struct EmptyDirectory;

#[async_trait]
impl UserDirectoryStore for EmptyDirectory {
    async fn observe(&self, _identity: &PublicIdentity) -> Result<(), StoreError> {
        Ok(())
    }

    async fn user(&self, _subject: &SubjectId) -> Result<Option<RegisteredUser>, StoreError> {
        Ok(None)
    }

    async fn list(&self, _query: &DirectoryQuery) -> Result<Page<RegisteredUser>, StoreError> {
        Ok(Page::new(Vec::new(), None))
    }
}

#[derive(Default)]
struct EmptyLaunchContext;

#[async_trait]
impl layer2_core::trigger_domain::LaunchContextReader for EmptyLaunchContext {
    async fn newest_matching_sandbox(
        &self,
        _owner: &SubjectId,
        _profile_id: &SandboxProfileId,
    ) -> Result<Option<layer2_core::sandbox::Sandbox>, LaunchResolutionError> {
        Ok(None)
    }

    async fn matching_workspace(
        &self,
        _owner: &SubjectId,
        _sandbox_id: &SandboxId,
        _profile_id: &WorkspaceProfileId,
    ) -> Result<Option<layer2_core::sandbox::Workspace>, LaunchResolutionError> {
        Ok(None)
    }

    async fn newest_live_trigger_agent(
        &self,
        _owner: &SubjectId,
        _trigger_id: &TriggerId,
    ) -> Result<Option<layer2_core::sandbox::Agent>, LaunchResolutionError> {
        Ok(None)
    }
}

#[derive(Default)]
struct ClosedConnectionBroker;

#[async_trait]
impl ServiceConnectionBroker for ClosedConnectionBroker {
    async fn requirements(
        &self,
        _owner: &SubjectId,
        _services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        Ok(Vec::new())
    }

    async fn create_handoff(
        &self,
        _owner: &SubjectId,
        _service: &ServiceId,
        _intent_id: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        Err(ServiceConnectionError::Refused)
    }

    async fn handoff_state(
        &self,
        _owner: &SubjectId,
        _external_id: &str,
        _service: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        Err(ServiceConnectionError::Refused)
    }
}

#[derive(Default)]
struct UnlimitedLaunchQuota;

#[async_trait]
impl LaunchQuota for UnlimitedLaunchQuota {
    async fn reserve(
        &self,
        _caller: &layer2_core::Caller,
    ) -> Result<LaunchQuotaPermit, LaunchQuotaError> {
        Ok(LaunchQuotaPermit::unmetered())
    }
}

struct MutableSettings(Mutex<StoredPlatformSettings>);

impl MutableSettings {
    fn new() -> Self {
        Self(Mutex::new(StoredPlatformSettings {
            changes: Default::default(),
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
            changes: Default::default(),
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
        Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let workspaces = WorkspaceService::new(Arc::clone(&sandbox_store), profile_store.clone());
    let projections = AgentProjectionServices::new(
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        Arc::clone(&sandbox_store),
        profile_store.clone(),
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
    let trigger_store = Arc::new(InMemoryTriggerStore::new(profile_store.clone()));
    let launch_store = Arc::new(InMemoryLaunchStore::new(Arc::clone(&trigger_store)));
    let broker = Arc::new(ClosedConnectionBroker);
    let triggers = TriggerService::new(
        Arc::clone(&trigger_store),
        Arc::clone(&launch_store),
        UserDirectory::new(Arc::new(EmptyDirectory)),
    );
    let launches = LaunchService::new(
        Arc::clone(&trigger_store),
        profile_store,
        Arc::clone(&launch_store),
        Arc::new(EmptyLaunchContext),
        Arc::clone(&broker),
        Arc::new(LaunchPolicy {
            enabled: true,
            orchestration_paused: false,
            visitor_launches_per_day: 5,
        }),
        Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    );
    let connections = ServiceConnectionService::new(Arc::clone(&launch_store), Arc::clone(&broker));
    let trigger_routes = trigger_router(TriggerRoutesState::new(
        triggers.clone(),
        launches,
        connections,
        Arc::clone(&operations),
        Arc::new(UnlimitedLaunchQuota),
    ))
    .merge(layer1_public::rest::agent_source_router(
        projections.reads(),
        triggers,
    ))
    .merge(trigger_assets_router(
        TriggerAssetsState::new(trigger_store, "https://cma.example").unwrap(),
    ))
    .merge(launch_companion_router(LaunchCompanionState::new(
        launch_store,
    )));
    rest_router(
        profiles_router(state)
            .merge(hierarchy_router(hierarchy))
            .merge(projections.router(operations))
            .merge(trigger_routes),
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
async fn cors_browser_control_and_replay_preflights_keep_exact_origin_policy() {
    let app = with_cors(
        Router::new(),
        vec![HeaderValue::from_static("https://console.example")],
    );
    for (origin, method, path, headers, allowed) in [
        (
            "https://console.example",
            "GET",
            "/api/v2/agents/one/events/snapshot",
            "authorization, Prefer",
            true,
        ),
        (
            "https://console.example",
            "POST",
            "/api/v1/agents/one/browser/control",
            "authorization, content-type, X-Cma-Browser-Client",
            true,
        ),
        (
            "https://console.example",
            "GET",
            "/api/v1/agents/one/browser/control",
            "authorization, x-cma-browser-client",
            true,
        ),
        (
            "https://unlisted.example",
            "GET",
            "/api/v2/agents/one/events",
            "authorization, prefer",
            false,
        ),
        (
            "https://console.example",
            "GET",
            "/api/v1/agents/one/answers/event:3/deliverables",
            "authorization, X-CMA-Document-Share-Token",
            true,
        ),
        (
            "https://console.example",
            "POST",
            "/api/v1/agents/one/browser/control",
            "authorization, x-cma-browser-client, x-unsupported",
            false,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri(path)
                    .header(header::ORIGIN, origin)
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, method)
                    .header(header::ACCESS_CONTROL_REQUEST_HEADERS, headers)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        if allowed {
            assert_eq!(
                response.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
                Some(&HeaderValue::from_static("https://console.example")),
                "{method} {path} with {headers}",
            );
            assert_eq!(
                response
                    .headers()
                    .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS),
                Some(&HeaderValue::from_static("true")),
            );
            let granted = response.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
                .to_str()
                .unwrap();
            for requested in headers.split(',') {
                assert!(granted
                    .split(',')
                    .any(|grant| grant.trim().eq_ignore_ascii_case(requested.trim())));
            }
        } else {
            assert!(!response
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
            assert!(!response
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_HEADERS));
        }
    }
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
async fn agent_routes_are_composed_before_the_rest_fallback() {
    let app = composed_rest_router();
    for (method, path, expected) in [
        (Method::GET, "/api/v1/agents", StatusCode::FORBIDDEN),
        (
            Method::GET,
            "/api/v1/agents/agt_missing",
            StatusCode::FORBIDDEN,
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

#[path = "main_tests/launch_subject.rs"]
mod launch_subject;
