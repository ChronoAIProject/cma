//! Retry the selected caller's GET without sharing another caller's authority.
use super::*;
use axum::body::Body;
use axum::http::Request;
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::store::InMemorySandboxStore;
use std::collections::{BTreeMap, VecDeque};
use tower::ServiceExt;

type Replies = VecDeque<(StatusCode, Value)>;

#[derive(Default)]
struct Provider {
    replies: RwLock<BTreeMap<String, Replies>>,
    seen: RwLock<Vec<(String, String)>>,
}

fn credential(headers: &HeaderMap) -> &str {
    headers[header::AUTHORIZATION]
        .to_str()
        .unwrap()
        .strip_prefix("Bearer ")
        .unwrap()
}

fn subject(token: &str) -> &'static str {
    match token {
        "fixture-a" => "human-a",
        "fixture-b" => "human-b",
        _ => panic!("unexpected credential"),
    }
}

async fn identity(headers: HeaderMap) -> Json<Value> {
    Json(json!({"sub":subject(credential(&headers)),"permissions":["cma:sandbox:use"]}))
}

async fn response(provider: Arc<Provider>, headers: HeaderMap, endpoint: String) -> Response {
    let token = credential(&headers).to_owned();
    provider.seen.write().await.push((token.clone(), endpoint));
    let reply = provider
        .replies
        .write()
        .await
        .get_mut(&token)
        .unwrap()
        .pop_front()
        .expect("no unplanned retry");
    (reply.0, Json(reply.1)).into_response()
}

async fn choices(State(provider): State<Arc<Provider>>, headers: HeaderMap) -> Response {
    response(provider, headers, "/api/v1/orgs".into()).await
}

async fn membership(
    State(provider): State<Arc<Provider>>,
    Path(org): Path<String>,
    headers: HeaderMap,
) -> Response {
    response(
        provider,
        headers,
        format!("/api/v1/orgs/{org}/authorization"),
    )
    .await
}

struct TwoManagers {
    provider: Arc<Provider>,
    auth: [Arc<AuthState>; 2],
    apps: [Router; 2],
    server: tokio::task::JoinHandle<()>,
}

impl Drop for TwoManagers {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl TwoManagers {
    async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let provider = Arc::new(Provider::default());
        let app = Router::new()
            .route("/userinfo", get(identity))
            .route("/api/v1/orgs", get(choices))
            .route("/api/v1/orgs/{org}/authorization", get(membership))
            .with_state(provider.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let auth = std::array::from_fn(|_| {
            AuthState::in_memory(
                AuthMode::Provider {
                    base_url: base.clone(),
                    client_id: "fixture".into(),
                    client_secret: "fixture-secret".into(),
                    public_url: "https://cma.test".into(),
                },
                None,
            )
        });
        for auth in &auth {
            *auth.metadata.write().await = Some(crate::auth::ProviderMetadata {
                authorization_endpoint: format!("{base}/authorize"),
                token_endpoint: format!("{base}/token"),
                userinfo_endpoint: Some(format!("{base}/userinfo")),
                introspection_endpoint: None,
            });
        }
        let apps = std::array::from_fn(|index| {
            let service = ResourceAccessService::new(
                Arc::new(InMemorySandboxStore::default()),
                auth[index].clone(),
            );
            layer1_public::rest::resource_access_router(Arc::new(service)).layer(
                axum::middleware::from_fn_with_state(
                    auth[index].clone(),
                    crate::auth::attach_subject,
                ),
            )
        });
        Self {
            provider,
            auth,
            apps,
            server,
        }
    }

    async fn replies(&self, token: &str, replies: impl Into<Replies>) {
        self.provider
            .replies
            .write()
            .await
            .insert(token.into(), replies.into());
    }

    async fn request(&self, manager: usize, token: &str) -> (StatusCode, Value) {
        let request = Request::get("/api/v1/organizations")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(3),
            self.apps[manager].clone().oneshot(request),
        )
        .await
        .unwrap()
        .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn lookup(&self, manager: usize, token: &str) -> Result<CallerContext, MembershipError> {
        let caller = Caller::new(
            SubjectId::new(subject(token)),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        tokio::time::timeout(
            Duration::from_secs(3),
            self.auth[manager].organization_context(&caller, token, &SubjectId::new("org-a")),
        )
        .await
        .unwrap()
    }

    async fn close(mut self) {
        self.server.abort();
        assert!(matches!((&mut self.server).await, Err(error) if error.is_cancelled()));
    }
}

#[tokio::test]
async fn concurrent_distinct_credentials_keep_retries_and_results_isolated() {
    let f = TwoManagers::new().await;
    f.replies(
        "fixture-a",
        [
            (StatusCode::SERVICE_UNAVAILABLE, json!({})),
            (
                StatusCode::OK,
                json!({"orgs":[{"id":"org-a","your_role":"admin"}]}),
            ),
        ],
    )
    .await;
    f.replies("fixture-b", [(StatusCode::FORBIDDEN, json!({}))])
        .await;
    let (a, b) = tokio::join!(f.request(0, "fixture-a"), f.request(1, "fixture-b"));
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(a.1["items"][0]["id"], "org-a");
    assert_eq!(b.0, StatusCode::FORBIDDEN);
    assert_eq!(b.1["code"], "resource_forbidden");
    assert!(b.1.get("items").is_none());
    let seen = f.provider.seen.read().await.clone();
    assert_eq!(
        seen.iter()
            .filter(|(token, _)| token == "fixture-a")
            .count(),
        2
    );
    assert_eq!(
        seen.iter()
            .filter(|(token, _)| token == "fixture-b")
            .count(),
        1
    );
    assert!(seen.iter().all(|(_, endpoint)| endpoint == "/api/v1/orgs"));

    // Reverse managers and membership dispositions: completed results cannot
    // become either credential's next organization authority.
    f.replies("fixture-a", [(StatusCode::FORBIDDEN, json!({}))])
        .await;
    f.replies(
        "fixture-b",
        [(
            StatusCode::OK,
            json!({"orgs":[{"id":"org-b","your_role":"viewer"}]}),
        )],
    )
    .await;
    let (a, b) = tokio::join!(f.request(1, "fixture-a"), f.request(0, "fixture-b"));
    assert_eq!(a.0, StatusCode::FORBIDDEN);
    assert_eq!(b.0, StatusCode::OK);
    assert_eq!(b.1["items"][0]["id"], "org-b");
    assert_eq!(f.provider.seen.read().await.len(), 5);
    f.close().await;
}

#[tokio::test]
async fn exact_membership_retry_preserves_endpoint_credential_role_and_revocation() {
    let f = TwoManagers::new().await;
    for (manager, token) in [(0, "fixture-a"), (1, "fixture-b")] {
        for role in ["admin", "viewer"] {
            f.replies(
                token,
                [
                    (StatusCode::SERVICE_UNAVAILABLE, json!({})),
                    (StatusCode::OK, json!({"id":"org-a","your_role":role})),
                ],
            )
            .await;
            let before = f.provider.seen.read().await.len();
            let context = f.lookup(manager, token).await.unwrap();
            assert_eq!(context.caller.subject.as_str(), subject(token));
            assert_eq!(context.memberships.len(), 1);
            assert_eq!(context.memberships[0].organization.as_str(), "org-a");
            assert_eq!(
                context.memberships[0].role,
                if role == "admin" {
                    OrganizationRole::Admin
                } else {
                    OrganizationRole::Viewer
                }
            );
            assert!(context.memberships[0].is_current(&context.credential, SystemTime::now()));
            let seen = f.provider.seen.read().await;
            assert_eq!(seen.len() - before, 2);
            assert!(seen[before..]
                .iter()
                .all(|(actual, endpoint)| actual == token
                    && endpoint == "/api/v1/orgs/org-a/authorization"));
        }
        f.replies(
            token,
            [
                (StatusCode::SERVICE_UNAVAILABLE, json!({})),
                (StatusCode::FORBIDDEN, json!({})),
            ],
        )
        .await;
        assert_eq!(
            f.lookup(manager, token).await,
            Err(MembershipError::NotMember)
        );
        f.replies(
            token,
            [
                (StatusCode::BAD_GATEWAY, json!({})),
                (
                    StatusCode::OK,
                    json!({"id":"other-org","your_role":"admin"}),
                ),
            ],
        )
        .await;
        assert_eq!(
            f.lookup(manager, token).await,
            Err(MembershipError::InvalidEvidence)
        );
    }
    f.close().await;
}
