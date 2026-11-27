use super::super::super::browser_rejection_tests::NoOperations;
use super::*;
use axum::body::Body;
use axum::http::{Method, Request};
use axum::middleware::{self, Next};
use layer1_public::rest::{
    hierarchy_router, personal_agent_inventory_router, HierarchyState, PersonalAgentInventory,
};
use layer1_public::{AuthenticatedCaller, DelegatedCredential};
use layer2_core::profiles::store::InMemoryProfileStore;
use layer2_core::sandbox::store::{AgentReadQueryStore, Page};
use layer2_core::sandbox::{
    Agent, AgentListQuery, AgentReadError, FixedSandboxCredentialPrerequisite,
    HierarchyReadService, SandboxPolicy, SandboxService, WorkspaceService,
};
use tower::ServiceExt as _;

struct Inventory(Arc<InMemorySandboxStore>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ObservedAuthority {
    sandbox_use: bool,
    delegated: bool,
    source: CredentialSource,
}

async fn observe_authority(request: Request<Body>, next: Next) -> Response {
    let extensions = request.extensions();
    let context = extensions.get::<CallerContext>().unwrap();
    let observed = ObservedAuthority {
        sandbox_use: context
            .caller
            .authorization
            .allows(Capability::UseOwnSandboxes),
        delegated: extensions.get::<DelegatedCredential>().is_some(),
        source: context.credential_source,
    };
    let mut response = next.run(request).await;
    response.extensions_mut().insert(observed);
    response
}

#[async_trait::async_trait]
impl PersonalAgentInventory for Inventory {
    async fn list(
        &self,
        caller: &Caller,
        query: &AgentListQuery,
        page: &PageRequest,
    ) -> Result<Page<Agent>, AgentReadError> {
        caller
            .require(Capability::UseOwnSandboxes)
            .map_err(|_| AgentReadError::Forbidden)?;
        Ok(self
            .0
            .list_owned_agents(&caller.subject, query, page)
            .await
            .unwrap())
    }
}

async fn send(app: &Router, method: Method, uri: &str, claims: &Value) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    *request.headers_mut() = signed_headers(claims);
    app.clone().oneshot(request).await.unwrap()
}

fn sandbox_inventory(fixture: &ResourceFixture) -> Router {
    let profiles = Arc::new(InMemoryProfileStore::default());
    hierarchy_router(HierarchyState::new(
        SandboxService::new(
            fixture.store.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(fixture.store.clone(), profiles),
        HierarchyReadService::new(fixture.store.clone()),
        Arc::new(NoOperations),
    ))
}

#[tokio::test]
async fn signed_personal_inventories_survive_explicit_retained_credential_rejection() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let ingress = SignedIngress::new(&fixture).await;
    let subject = SubjectId::new("owner");
    let control = &fixture.identity.control;
    let app = personal_agent_inventory_router(Arc::new(Inventory(fixture.store.clone())))
        .merge(sandbox_inventory(&fixture))
        .layer(middleware::from_fn(observe_authority))
        .layer(middleware::from_fn_with_state(
            ingress.auth.clone(),
            crate::auth::attach_subject,
        ));
    let paths = [
        ("/api/v1/agents?limit=1", "/items/0/id", "agent"),
        (
            "/api/v1/sandboxes?limit=1&exclude_retired=true&shared=false",
            "/items/0/id",
            "sandbox",
        ),
    ];

    for rejected_by_userinfo in [false, true] {
        if rejected_by_userinfo {
            *control.introspection.write().await =
                Reply::json(json!({"active":true,"sub":"owner","permissions":["cma:sandbox:use"]}));
            control.userinfo.write().await.status = StatusCode::UNAUTHORIZED;
        } else {
            *control.introspection.write().await = Reply::json(json!({"active":false}));
        }
        for (uri, id_pointer, id) in paths {
            let baseline = control.requests.load(Ordering::SeqCst);
            let claims = signed_claims();
            let response = send(&app, Method::GET, uri, &claims).await;
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            let observed = response.extensions().get::<ObservedAuthority>().unwrap();
            assert!(matches!(
                observed.source,
                CredentialSource::Independent { .. }
            ));
            assert!(observed.sandbox_use);
            assert!(!observed.delegated);
            let bytes = axum::body::to_bytes(response.into_body(), 16384)
                .await
                .unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body.pointer(id_pointer).and_then(Value::as_str), Some(id));
            assert!(control.requests.load(Ordering::SeqCst) > baseline);
            let grant =
                ProxyGrant::identity(ISSUER, SERVICE, &subject, claims["jti"].as_str().unwrap());
            assert!(fixture
                .identity
                .store
                .proxy_grant(&grant)
                .await
                .unwrap()
                .is_none());

            let mut unprivileged = signed_claims();
            unprivileged["permissions"] = json!([]);
            assert_eq!(
                send(&app, Method::GET, uri, &unprivileged).await.status(),
                StatusCode::FORBIDDEN,
                "{uri}"
            );
        }
    }
    for (method, uri) in [
        (Method::GET, "/api/v1/agents?%6Frganization=org"),
        (Method::GET, "/api/v1/sandboxes?%6Frganization=org"),
        (Method::GET, "/api/v1/sandboxes?%73hared=tr%75e"),
        (Method::POST, "/api/v1/sandboxes"),
    ] {
        let response = send(&app, method, uri, &signed_claims()).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{uri}");
        assert!(response.extensions().get::<ObservedAuthority>().is_none());
    }

    control.userinfo.write().await.status = StatusCode::OK;
    *control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":[]}));
    for (uri, _, _) in paths {
        let response = send(&app, Method::GET, uri, &signed_claims()).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{uri}");
        let observed = response.extensions().get::<ObservedAuthority>().unwrap();
        assert!(matches!(
            observed.source,
            CredentialSource::IdentityProxy { .. }
        ));
        assert!(!observed.sandbox_use);
        assert!(observed.delegated);
    }
    *control.introspection.write().await = Reply::json(json!({"active":"unknown"}));
    for (uri, _, _) in paths {
        let response = send(&app, Method::GET, uri, &signed_claims()).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{uri}");
        assert!(response.extensions().get::<ObservedAuthority>().is_none());
    }
}

#[tokio::test]
async fn signed_personal_inventory_uses_assertion_when_retained_tokens_are_unusable() {
    let fixture = ResourceFixture::with_owner("owner").await;
    let ingress = SignedIngress::new(&fixture).await;
    let subject = SubjectId::new("owner");
    let baseline = fixture.identity.control.requests.load(Ordering::SeqCst);
    let app = personal_agent_inventory_router(Arc::new(Inventory(fixture.store.clone())))
        .route(
            "/api/v1/agents",
            post(
                |AuthenticatedCaller(caller): AuthenticatedCaller,
                 credential: Option<axum::Extension<DelegatedCredential>>| async move {
                    assert!(credential.is_none());
                    assert!(!caller.authorization.allows(Capability::UseOwnSandboxes));
                    StatusCode::FORBIDDEN
                },
            ),
        )
        .layer(middleware::from_fn(observe_authority))
        .layer(middleware::from_fn_with_state(
            ingress.auth.clone(),
            crate::auth::attach_subject,
        ));

    let mut owner = fixture
        .identity
        .store
        .subject(&subject)
        .await
        .unwrap()
        .unwrap();
    owner.tokens = None;
    owner.token_generation += 1;
    owner.revision += 1;
    fixture.identity.store.put_subject(owner).await.unwrap();

    let claims = signed_claims();
    let response = send(&app, Method::GET, "/api/v1/agents?limit=1", &claims).await;
    assert_eq!(response.status(), StatusCode::OK);
    let observed = response.extensions().get::<ObservedAuthority>().unwrap();
    assert!(matches!(
        observed.source,
        CredentialSource::Independent { .. }
    ));
    assert!(observed.sandbox_use);
    assert!(!observed.delegated);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["id"], "agent");
    let grant_id = ProxyGrant::identity(ISSUER, SERVICE, &subject, claims["jti"].as_str().unwrap());
    assert!(fixture
        .identity
        .store
        .proxy_grant(&grant_id)
        .await
        .unwrap()
        .is_none());

    let mut unprivileged = signed_claims();
    unprivileged["permissions"] = json!([]);
    let refusal = send(&app, Method::GET, "/api/v1/agents", &unprivileged).await;
    assert_eq!(refusal.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(refusal.into_body(), 4096)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["type"]
        .as_str()
        .unwrap()
        .ends_with("/sandbox-use-forbidden"));

    let organization = signed_claims();
    let response = send(
        &app,
        Method::GET,
        "/api/v1/agents?%6Frganization=org",
        &organization,
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let grant_digest = ProxyGrant::identity(
        ISSUER,
        SERVICE,
        &subject,
        organization["jti"].as_str().unwrap(),
    );
    assert_eq!(
        response.extensions().get::<ObservedAuthority>().unwrap(),
        &ObservedAuthority {
            sandbox_use: false,
            delegated: false,
            source: CredentialSource::IdentityProxy { grant_digest },
        },
    );
    assert_eq!(
        send(&app, Method::POST, "/api/v1/agents", &signed_claims())
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture.identity.control.requests.load(Ordering::SeqCst),
        baseline
    );

    fixture.identity.seed().await;
    *fixture.identity.control.introspection.write().await =
        Reply::json(json!({"active":true,"sub":"owner","permissions":[]}));
    let active_without_permission =
        send(&app, Method::GET, "/api/v1/agents", &signed_claims()).await;
    assert_eq!(active_without_permission.status(), StatusCode::FORBIDDEN);
    assert!(fixture.identity.control.requests.load(Ordering::SeqCst) > baseline);
}
