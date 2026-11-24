use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use layer2_core::resource_access::{
    AccessDecision, AccessPurpose, ResourceCapability, ResourceOwner, ResourceScope,
    SharingIsolation,
};
use layer2_core::{Authorization, Capability};
use serde_json::{json, Value};
use tokio::sync::{RwLock, Semaphore};

use super::*;
use crate::auth::AuthMode;

struct Control {
    subject: RwLock<String>,
    reply: RwLock<(StatusCode, Value)>,
    inventory: RwLock<Value>,
    lookups: AtomicUsize,
    inventory_calls: AtomicUsize,
    userinfo_calls: AtomicUsize,
    introspection_calls: AtomicUsize,
    introspection: RwLock<Value>,
    verification_gate: RwLock<Option<Arc<Semaphore>>>,
    verification_arrivals: Semaphore,
    delay: RwLock<Duration>,
}
async fn userinfo(State(control): State<Arc<Control>>, headers: HeaderMap) -> Json<Value> {
    assert_eq!(headers[header::AUTHORIZATION], "Bearer fixture-credential");
    control.userinfo_calls.fetch_add(1, Ordering::SeqCst);
    Json(json!({"sub":*control.subject.read().await,"permissions":["cma:sandbox:use"]}))
}
async fn introspection(State(control): State<Arc<Control>>) -> Json<Value> {
    control.introspection_calls.fetch_add(1, Ordering::SeqCst);
    control.verification_arrivals.add_permits(1);
    let gate = control.verification_gate.read().await.clone();
    if let Some(gate) = gate {
        gate.acquire().await.unwrap().forget();
    }
    Json(control.introspection.read().await.clone())
}
async fn authorization(
    State(control): State<Arc<Control>>,
    Path(org): Path<String>,
    headers: HeaderMap,
) -> Response {
    assert_eq!(headers[header::AUTHORIZATION], "Bearer fixture-credential");
    control.lookups.fetch_add(1, Ordering::SeqCst);
    let (status, mut body) = control.reply.read().await.clone();
    if body.get("id").is_none() {
        body["id"] = json!(org);
    }
    tokio::time::sleep(*control.delay.read().await).await;
    (status, Json(body)).into_response()
}
async fn inventory(State(control): State<Arc<Control>>) -> Json<Value> {
    control.inventory_calls.fetch_add(1, Ordering::SeqCst);
    Json(control.inventory.read().await.clone())
}
struct Fixture {
    auth: Arc<AuthState>,
    control: Arc<Control>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let control = Arc::new(Control {
            subject: RwLock::new("human".into()),
            reply: RwLock::new((StatusCode::OK, json!({"your_role":"admin"}))),
            inventory: RwLock::new(
                json!({"orgs":[{"id":"org-b","your_role":"viewer"},{"id":"org-a","your_role":"admin","display_name":"A"}]}),
            ),
            lookups: AtomicUsize::new(0),
            inventory_calls: AtomicUsize::new(0),
            userinfo_calls: AtomicUsize::new(0),
            introspection_calls: AtomicUsize::new(0),
            introspection: RwLock::new(
                json!({"active":true,"sub":"human","permissions":["cma:sandbox:use"]}),
            ),
            verification_gate: RwLock::new(None),
            verification_arrivals: Semaphore::new(0),
            delay: RwLock::new(Duration::ZERO),
        });
        let router = Router::new()
            .route("/userinfo", get(userinfo))
            .route("/introspect", post(introspection))
            .route("/api/v1/orgs/{org}/authorization", get(authorization))
            .route("/api/v1/orgs", get(inventory))
            .with_state(control.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let auth = AuthState::in_memory(
            AuthMode::Provider {
                base_url: base.clone(),
                client_id: "fixture".into(),
                client_secret: "fixture-secret".into(),
                public_url: "https://cma.test".into(),
            },
            None,
        );
        *auth.metadata.write().await = Some(super::super::ProviderMetadata {
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            userinfo_endpoint: Some(format!("{base}/userinfo")),
            introspection_endpoint: None,
        });
        Self {
            auth,
            control,
            server,
        }
    }
    async fn lookup(&self, org: &str) -> Result<CallerContext, MembershipError> {
        self.auth
            .organization_context(&caller(), "fixture-credential", &SubjectId::new(org))
            .await
    }
}

mod principal_failures;
mod verification;
fn caller() -> Caller {
    Caller::new(
        SubjectId::new("human"),
        Authorization::new([Capability::UseOwnSandboxes]),
    )
}

#[tokio::test]
async fn authenticated_request_context_matches_fresh_provider_evidence_without_extra_lookup() {
    let fixture = Fixture::new().await;
    let caller = caller();
    let resolved = crate::auth::RequestAuth {
        binding: None,
        credential_source: layer2_core::resource_access::CredentialSource::Retained,
        subject: caller.subject.clone(),
        authorization: caller.authorization.clone(),
        profile: crate::auth::profile::Profile::default(),
        delegated_credential: Some("fixture-credential".into()),
    };
    let request = fixture.auth.resource_request_context(&resolved);
    assert!(request.validate_principal());
    assert_eq!(request.caller, caller);
    assert!(request.memberships.is_empty());
    assert!(!format!("{request:?}").contains("fixture-credential"));
    assert_eq!(fixture.control.userinfo_calls.load(Ordering::SeqCst), 0);
    let fresh = fixture.lookup("org-a").await.unwrap();
    assert_eq!(fresh.credential, request.credential);
    assert_eq!(fresh.actor, request.actor);
}
fn scope() -> ResourceScope {
    ResourceScope {
        sandbox_id: layer2_core::SandboxId("sandbox".into()),
        agent_id: Some(layer2_core::AgentId("agent".into())),
        owner: ResourceOwner::Organization {
            subject: SubjectId::new("org-a"),
        },
        sharing_isolation: SharingIsolation::Unproved,
    }
}

#[tokio::test]
async fn live_role_changes_and_removal_affect_the_next_lookup_without_a_cache() {
    let fixture = Fixture::new().await;
    for (role, interact) in [("admin", true), ("member", false), ("viewer", false)] {
        *fixture.control.reply.write().await = (StatusCode::OK, json!({"your_role":role}));
        let started = SystemTime::now();
        let context = fixture.lookup("org-a").await.unwrap();
        let finished = SystemTime::now();
        let access = AccessDecision::evaluate(
            &context,
            &scope(),
            &[],
            AccessPurpose::Mutation {
                started_at: started,
            },
            finished,
        )
        .unwrap();
        assert_eq!(access.allows(ResourceCapability::Interact), interact);
        assert!(access.allows(ResourceCapability::ViewHistory));
        let evidence = &context.memberships[0];
        assert!((started..=finished).contains(&evidence.observed_at));
        assert_eq!(
            evidence.expires_at,
            evidence.observed_at + MAX_MEMBERSHIP_AGE
        );
    }
    assert_eq!(fixture.control.lookups.load(Ordering::SeqCst), 3);
    *fixture.control.reply.write().await = (StatusCode::NOT_FOUND, json!({}));
    assert_eq!(
        fixture.lookup("org-a").await,
        Err(MembershipError::NotMember)
    );
}

#[tokio::test]
async fn unknown_roles_wrong_canonical_ids_and_provider_failures_are_closed() {
    let fixture = Fixture::new().await;
    for body in [
        json!({"your_role":"owner"}),
        json!({"your_role":"ADMIN"}),
        json!({"your_role":"admin","id":"different"}),
        json!({"your_role":null}),
    ] {
        *fixture.control.reply.write().await = (StatusCode::OK, body);
        assert_eq!(
            fixture.lookup("org-a").await,
            Err(MembershipError::InvalidEvidence)
        );
    }
    *fixture.control.reply.write().await = (
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"secret":"provider detail must not propagate"}),
    );
    assert_eq!(
        fixture.lookup("org-a").await,
        Err(MembershipError::Unavailable)
    );
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::NOT_FOUND,
    ] {
        *fixture.control.reply.write().await = (status, json!({}));
        assert_eq!(
            fixture.lookup("org-a").await,
            Err(MembershipError::NotMember)
        );
    }
    assert!(
        AccessDecision::personal(&caller(), &caller().subject).allows(ResourceCapability::Interact)
    );
}

#[tokio::test]
async fn wrong_subject_is_refused_before_membership_and_issuers_are_distinct() {
    let first = Fixture::new().await;
    *first.control.subject.write().await = "other".into();
    assert_eq!(first.lookup("org-a").await, Err(MembershipError::NotMember));
    assert_eq!(first.control.lookups.load(Ordering::SeqCst), 0);
    *first.control.subject.write().await = "human".into();
    let second = Fixture::new().await;
    second.control.reply.write().await.1 = json!({"your_role":"viewer"});
    let a = first.lookup("org-a").await.unwrap();
    let b = second.lookup("org-a").await.unwrap();
    assert_ne!(a.credential.issuer, b.credential.issuer);
    assert!(!a.memberships[0].is_current(&b.credential, SystemTime::now()));
    let c = first.lookup("org-b").await.unwrap();
    assert_ne!(a.memberships[0].organization, c.memberships[0].organization);
}

#[tokio::test]
async fn inventory_is_sorted_validated_and_never_used_as_membership_authority() {
    let fixture = Fixture::new().await;
    let items = fixture
        .auth
        .organization_choices(&caller(), "fixture-credential")
        .await
        .unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        ["org-a", "org-b"]
    );
    assert_eq!(fixture.control.lookups.load(Ordering::SeqCst), 0);
    for body in [
        json!({"orgs":[{"id":"a","your_role":"admin"},{"id":"a","your_role":"viewer"}]}),
        json!({"orgs":[{"id":"a","your_role":"superadmin"}]}),
        json!({"orgs":[{"id":"../admin","your_role":"admin"}]}),
    ] {
        *fixture.control.inventory.write().await = body;
        assert_eq!(
            fixture
                .auth
                .organization_choices(&caller(), "fixture-credential")
                .await,
            Err(MembershipError::InvalidEvidence)
        );
    }
}

#[tokio::test]
async fn path_manipulation_and_restricted_credentials_do_not_reach_org_endpoints() {
    use base64::Engine as _;
    let fixture = Fixture::new().await;
    for org in ["..", "org/other", "org?key=other", "org%2Fother", ""] {
        assert_eq!(
            fixture.lookup(org).await,
            Err(MembershipError::InvalidEvidence)
        );
    }
    assert_eq!(fixture.control.lookups.load(Ordering::SeqCst), 0);
    let before = fixture.control.userinfo_calls.load(Ordering::SeqCst);
    for claims in [
        json!({"relay":true}),
        json!({"sa":true}),
        json!({"sa":"false"}),
        json!({"delegated":true}),
        json!({"act":{"sub":"owner"}}),
        json!({"token_type":"relay_reply"}),
    ] {
        let token = format!(
            "e30.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&claims).unwrap())
        );
        assert_eq!(
            fixture
                .auth
                .organization_context(&caller(), &token, &SubjectId::new("org-a"))
                .await,
            Err(MembershipError::NotMember)
        );
    }
    assert_eq!(
        fixture.control.userinfo_calls.load(Ordering::SeqCst),
        before
    );
}

#[tokio::test]
async fn slow_provider_time_counts_against_the_fixed_membership_lifetime() {
    let fixture = Fixture::new().await;
    *fixture.control.delay.write().await = Duration::from_millis(50);
    let started = SystemTime::now();
    let context = fixture.lookup("org-a").await.unwrap();
    let evidence = &context.memberships[0];
    assert!(evidence.observed_at >= started);
    assert!(evidence.observed_at + Duration::from_millis(40) < SystemTime::now());
    assert_eq!(
        evidence
            .expires_at
            .duration_since(evidence.observed_at)
            .unwrap(),
        MAX_MEMBERSHIP_AGE
    );
    assert!(!evidence.is_current(&context.credential, evidence.expires_at));
    assert!(!format!("{context:?}").contains("fixture-credential"));
}
