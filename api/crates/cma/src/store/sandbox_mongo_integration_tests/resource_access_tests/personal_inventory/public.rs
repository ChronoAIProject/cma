use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use layer2_core::sandbox::{
    AgentOperationService, AgentRuntimeConnection, AgentRuntimeError, AgentRuntimeRegistry,
    AgentRuntimeTarget, AgentService, SandboxResourceReadError, SandboxResourceReader,
    SandboxResourceReading,
};
use tower::ServiceExt;

pub(super) struct NoExternal;
#[async_trait::async_trait]
impl AgentRuntimeRegistry for NoExternal {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        panic!("a list must not contact a runtime")
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        panic!("a list must not fence runtime work")
    }
}
#[async_trait::async_trait]
impl SandboxResourceReader for NoExternal {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("a list must not inspect runtime resources")
    }
}
#[async_trait::async_trait]
impl OrganizationAuthority for NoExternal {
    async fn organization_context(
        &self,
        _: &Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::NotMember)
    }
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("personal list must not enumerate organizations")
    }
}
impl fixture::Fixture {
    pub fn app_with_access(
        &self,
        store: Arc<MongoSandboxStore>,
        access: Arc<dyn ResourceShareStore>,
        joined: bool,
    ) -> Router {
        self.app_with_authority(store, access, joined, Arc::new(NoExternal))
    }
    pub fn app_with_authority(
        &self,
        store: Arc<MongoSandboxStore>,
        access: Arc<dyn ResourceShareStore>,
        joined: bool,
        organizations: Arc<dyn OrganizationAuthority>,
    ) -> Router {
        let access = Arc::new(ResourceAccessService::new(access, organizations));
        let inventory = layer2_core::sandbox::AgentProvenanceInventoryService::new(
            store.clone(),
            access.clone(),
        );
        let reads = AgentReadService::new(store.clone(), Arc::new(NoExternal))
            .with_resource_access(access.clone());
        let agents = AgentService::new(
            store.clone(),
            Arc::new(layer2_core::profiles::InMemoryProfileStore::default()),
            Arc::new(NoExternal),
            layer2_core::RuntimeCapabilities::new([]),
        );
        let operations = AgentOperationService::new(store, Arc::new(NoExternal));
        layer1_public::rest::agent_router(layer1_public::rest::AgentRoutesState::new(
            layer1_public::agent_page::AgentPageOrigin::new("https://example.test").unwrap(),
            agents,
            reads,
            operations,
            self.operations.clone(),
        ))
        .merge(layer1_public::rest::agent_provenance_inventory_router(
            inventory,
        ))
        .merge(layer1_public::rest::resource_access_router(access))
        .layer(axum::middleware::from_fn_with_state(
            self.activity.clone(),
            crate::activity::record,
        ))
        .layer(axum::middleware::from_fn_with_state(
            self.limits.clone(),
            crate::rate_limit::enforce,
        ))
        .layer(axum::middleware::from_fn_with_state(
            crate::expensive_reads::ReadBudget::new(false),
            crate::expensive_reads::enforce,
        ))
        .layer(axum::middleware::from_fn_with_state(
            self.identity.auth(joined),
            crate::auth::attach_subject,
        ))
        .layer(axum::middleware::from_fn(
            crate::background::interactive_request,
        ))
        .layer(axum::middleware::from_fn_with_state(
            self.limits.clone(),
            crate::rate_limit::admit,
        ))
    }
}
pub(super) async fn request(app: &Router, path: &str) -> (StatusCode, serde_json::Value) {
    request_cookie(app, path, "agent-list-cookie").await
}
pub(super) async fn request_cookie(
    app: &Router,
    path: &str,
    cookie: &str,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::get(path)
                .header(header::COOKIE, format!("cma_session={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        tracing::debug_span!(target: "cma.agent_inventory", "client_json_decode")
            .in_scope(|| serde_json::from_slice(&bytes))
            .unwrap(),
    )
}
