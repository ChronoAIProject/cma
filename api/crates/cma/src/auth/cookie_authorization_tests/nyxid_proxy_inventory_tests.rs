use super::*;
use axum::body::Body;
use axum::http::{Method, Request};
use axum::middleware::{self, Next};
use layer1_public::rest::{personal_agent_inventory_router, PersonalAgentInventory};
use layer1_public::{AuthenticatedCaller, DelegatedCredential};
use layer2_core::sandbox::store::{AgentReadQueryStore, Page};
use layer2_core::sandbox::{Agent, AgentListQuery, AgentReadError};
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
