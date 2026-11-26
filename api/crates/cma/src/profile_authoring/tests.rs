use std::collections::BTreeMap;
use std::sync::Mutex;

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use serde_json::{json, Value};

use super::*;

const SKILLS: &str = "/api/v1/proxy/s/ornn-api/api/v1";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const TAG_SHA: &str = "abcdef0123456789abcdef0123456789abcdef01";

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    query: Option<String>,
    authorization: Option<String>,
}

type Responses = Arc<Mutex<BTreeMap<String, (u16, Vec<u8>)>>>;

#[derive(Clone, Default)]
struct UpstreamState {
    responses: Responses,
    seen: Arc<Mutex<Vec<Seen>>>,
}

struct Upstream {
    origin: reqwest::Url,
    state: UpstreamState,
    task: tokio::task::JoinHandle<()>,
}

impl Upstream {
    async fn start() -> Self {
        let state = UpstreamState::default();
        let router = Router::new().fallback(reply).with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            origin: reqwest::Url::parse(&format!("http://{address}")).unwrap(),
            state,
            task,
        }
    }

    fn response(&self, path: &str, status: u16, value: Value) {
        self.state
            .responses
            .lock()
            .unwrap()
            .insert(path.into(), (status, serde_json::to_vec(&value).unwrap()));
    }

    fn seen(&self) -> Vec<Seen> {
        self.state.seen.lock().unwrap().clone()
    }

    fn source(&self, tokens: Option<Arc<TestTokens>>) -> CmaProfileAuthoringSource {
        let http = http::client().unwrap();
        CmaProfileAuthoringSource {
            issuer: tokens.as_ref().map(|_| {
                Arc::new(KeyIssuer::new(
                    http.clone(),
                    self.origin.as_str(),
                    "authoring-test",
                ))
            }),
            tokens: tokens.map(|tokens| tokens as Arc<dyn OwnerTokenSource>),
            skills: skills::SkillCatalog {
                http: http.clone(),
                origin: self.origin.clone(),
            },
            repositories: repositories::RepositoryResolver {
                http,
                origin: self.origin.clone(),
            },
        }
    }

    fn repository(&self) {
        self.response(
            "/repos/the-omega-institute/trureturing",
            200,
            json!({
                "full_name":"the-omega-institute/trureturing",
                "html_url":"https://github.com/the-omega-institute/trureturing",
                "default_branch":"main", "private":false, "disabled":false
            }),
        );
    }

    fn reference(&self, kind: &str, reference: &str, object_kind: &str, sha: &str) {
        let mut url = self.origin.clone();
        url.path_segments_mut().unwrap().extend([
            "repos",
            "the-omega-institute",
            "trureturing",
            "git",
            "ref",
            kind,
            reference,
        ]);
        self.response(
            url.path(),
            200,
            json!({
                "ref":format!("refs/{kind}/{reference}"),
                "object":{"type":object_kind,"sha":sha}
            }),
        );
    }

    fn skills(&self, items: Value, has_more: bool, next: Option<&str>) {
        self.response(
            &format!("{SKILLS}/skill-search"),
            200,
            json!({
                "data":{"items":items,"meta":{"hasMore":has_more,"nextCursor":next}},
                "error":null
            }),
        );
    }

    fn versions(&self, id: &str, items: Value) {
        self.response(
            &format!("{SKILLS}/skills/{id}/versions"),
            200,
            json!({
                "data":{"items":items}, "error":null
            }),
        );
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn reply(State(state): State<UpstreamState>, request: Request<Body>) -> Response<Body> {
    let path = request.uri().path().to_owned();
    state.seen.lock().unwrap().push(Seen {
        path: path.clone(),
        query: request.uri().query().map(str::to_owned),
        authorization: request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned),
    });
    let (status, body) = state
        .responses
        .lock()
        .unwrap()
        .get(&path)
        .cloned()
        .unwrap_or((404, Vec::new()));
    // Deliberately stream without Content-Length so the byte limit is enforced
    // during reads, not only against an optional upstream size announcement.
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
        body.chunks(4096).map(|chunk| Ok(chunk.to_vec())).collect();
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .body(Body::from_stream(futures_util::stream::iter(chunks)))
        .unwrap()
}

struct TestTokens {
    value: Option<String>,
    owners: Mutex<Vec<SubjectId>>,
}

impl TestTokens {
    fn new(value: Option<&str>) -> Arc<Self> {
        Arc::new(Self {
            value: value.map(str::to_owned),
            owners: Mutex::default(),
        })
    }
}

#[async_trait::async_trait]
impl OwnerTokenSource for TestTokens {
    async fn token_for(&self, owner: &SubjectId) -> Result<Option<SecretValue>, String> {
        self.owners.lock().unwrap().push(owner.clone());
        Ok(self.value.clone().map(SecretValue::new))
    }
}

fn owner() -> SubjectId {
    SubjectId::new("profile-owner")
}

fn query() -> SkillCatalogQuery {
    SkillCatalogQuery {
        query: "research & tools".into(),
        limit: 20,
        cursor: None,
    }
}

fn repository(reference: Option<&str>) -> RepositoryRequest {
    RepositoryRequest {
        url: "https://github.com/the-omega-institute/trureturing".into(),
        reference: reference.map(str::to_owned),
    }
}

#[tokio::test]
async fn public_authoring_routes_reach_production_adapters_through_core_and_cma_auth() {
    use layer2_core::profiles::authoring::ProfileAuthoringService;
    use tower::ServiceExt as _;

    let upstream = Upstream::start().await;
    upstream.repository();
    upstream.reference("heads", "main", "commit", SHA);
    upstream.response(
        "/api/v1/services",
        200,
        json!({"services":[{
            "id":"service/a ?#","slug":"research","name":"Research","is_active":true
        }]}),
    );
    upstream.skills(json!([{"guid":"research","name":"Research"}]), false, None);
    upstream.versions("research", json!([{"version":"1.0","isDeprecated":false}]));
    let tokens = TestTokens::new(Some("owner-catalogue-reader"));
    let service = ProfileAuthoringService::new(
        crate::profile_policy::profile_route_policy().capacity_bounds,
        Arc::new(upstream.source(Some(tokens.clone()))),
    );
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: owner().to_string(),
        },
        None,
    );
    let router = crate::rest_router(layer1_public::rest::profile_authoring_router(
        service,
        Some("https://catalog.example.test".parse().unwrap()),
    ))
    .layer(axum::middleware::from_fn_with_state(
        auth,
        crate::auth::attach_subject,
    ));
    let requests = [
        (
            "GET",
            "bounds",
            None,
            json!({"min_cpu_millis":100,"max_cpu_millis":8000,
            "min_memory_mib":128,"max_memory_mib":16384}),
        ),
        (
            "GET",
            "services",
            None,
            json!([{"id":"research","catalog_id":"service/a ?#","name":"Research",
                "detail_url":"https://catalog.example.test/services/service%2Fa%20%3F%23"}]),
        ),
        (
            "GET",
            "skills",
            None,
            json!({"items":[{"id":"research","name":"Research","versions":["1.0"]}]}),
        ),
        (
            "POST",
            "repositories/resolve",
            Some(json!({"url":repository(None).url})),
            json!({"url":repository(None).url,"default_branch":"main","commit_sha":SHA}),
        ),
    ];
    for (method, suffix, body, expected) in requests {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(format!("/api/v1/profile-authoring/{suffix}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        body.map_or_else(String::new, |value| value.to_string()),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{suffix}");
        assert!(response.headers().contains_key("link"));
        let bytes = axum::body::to_bytes(response.into_body(), 100_000)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            expected,
            "{suffix}"
        );
    }
    assert_eq!(*tokens.owners.lock().unwrap(), vec![owner(), owner()]);
    assert_eq!(upstream.seen().len(), 5);
}

#[tokio::test]
async fn bearer_only_public_clients_can_read_catalogues_without_a_stored_browser_login() {
    use layer2_core::profiles::authoring::ProfileAuthoringService;
    use tower::ServiceExt as _;

    let upstream = Upstream::start().await;
    upstream.response(
        "/.well-known/openid-configuration",
        200,
        json!({
            "authorization_endpoint":upstream.origin.join("authorize").unwrap().as_str(),
            "token_endpoint":upstream.origin.join("token").unwrap().as_str(),
            "userinfo_endpoint":upstream.origin.join("userinfo").unwrap().as_str()
        }),
    );
    upstream.response(
        "/userinfo",
        200,
        json!({
            "sub":owner().as_str(),"permissions":["cma:profile:manage"]
        }),
    );
    upstream.response(
        "/api/v1/services",
        200,
        json!({"services":[{
            "id":"service-a","slug":"research","name":"Research","is_active":true
        }]}),
    );
    upstream.skills(json!([{"guid":"research","name":"Research"}]), false, None);
    upstream.versions("research", json!([{"version":"1.0","isDeprecated":false}]));
    upstream.repository();
    upstream.reference("heads", "main", "commit", SHA);
    let auth = AuthState::in_memory(
        AuthMode::Provider {
            base_url: upstream.origin.as_str().trim_end_matches('/').into(),
            client_id: "authoring-client".into(),
            client_secret: "authoring-client-value".into(),
            public_url: "https://cma.example.test".into(),
        },
        None,
    );
    assert!(auth
        .delegated_credential_for(&owner())
        .await
        .unwrap()
        .is_none());
    let mut source = upstream.source(None);
    source.issuer = Some(Arc::new(KeyIssuer::new(
        http::client().unwrap(),
        upstream.origin.as_str(),
        "authoring-test",
    )));
    source.tokens = Some(auth.clone());
    let service = ProfileAuthoringService::new(
        crate::profile_policy::profile_route_policy().capacity_bounds,
        Arc::new(source),
    );
    let router =
        crate::rest_router(layer1_public::rest::profile_authoring_router(service, None)).layer(
            axum::middleware::from_fn_with_state(auth.clone(), crate::auth::attach_subject),
        );
    for suffix in ["services", "skills"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/profile-authoring/{suffix}"))
                    .header("authorization", "Bearer request-scoped-catalogue")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{suffix}");
    }
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/profile-authoring/repositories/resolve")
                .header("authorization", "Bearer request-scoped-catalogue")
                .header("content-type", "application/json")
                .body(Body::from(json!({"url":repository(None).url}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(auth
        .delegated_credential_for(&owner())
        .await
        .unwrap()
        .is_none());
    let seen = upstream.seen();
    for row in seen
        .iter()
        .filter(|row| row.path == "/api/v1/services" || row.path.starts_with(SKILLS))
    {
        assert_eq!(
            row.authorization.as_deref(),
            Some("Bearer request-scoped-catalogue")
        );
    }
    assert!(seen
        .iter()
        .filter(|row| row.path.starts_with("/repos/"))
        .all(|row| row.authorization.is_none()));
    let calls = seen.len();
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/profile-authoring/services")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(upstream.seen().len(), calls);
}

#[tokio::test]
async fn supplied_catalogue_authority_never_falls_back_to_broader_stored_authority() {
    let upstream = Upstream::start().await;
    upstream.response(
        "/api/v1/services",
        403,
        json!({"error":"scoped credential refused"}),
    );
    upstream.response(
        &format!("{SKILLS}/skill-search"),
        403,
        json!({"error":"scoped credential refused"}),
    );
    let tokens = TestTokens::new(Some("broader-stored-owner-authority"));
    let source = upstream.source(Some(tokens.clone()));
    let scoped = AuthoringCredential::new("limited-request-authority".into());
    assert!(matches!(
        source.services(&owner(), Some(&scoped)).await,
        Err(AuthoringSourceError::Unavailable(_))
    ));
    assert!(matches!(
        source.skills(&owner(), &query(), Some(&scoped)).await,
        Err(AuthoringSourceError::Unavailable(_))
    ));
    assert!(tokens.owners.lock().unwrap().is_empty());
    assert_eq!(upstream.seen().len(), 2);
    assert!(upstream
        .seen()
        .iter()
        .all(|row| row.authorization.as_deref() == Some("Bearer limited-request-authority")));
}

#[tokio::test]
async fn services_use_owner_authority_and_project_only_catalogue_metadata() {
    let upstream = Upstream::start().await;
    upstream.response(
        "/api/v1/services",
        200,
        json!({"services":[{
            "id":"service-uuid", "slug":"research", "name":"Research", "description":"Read papers",
            "is_active":true, "viewer_routing":{"private":"not public profile data"}
        }]}),
    );
    let tokens = TestTokens::new(Some("catalogue-owner-authority"));
    let result = upstream
        .source(Some(tokens.clone()))
        .services(&owner(), None)
        .await
        .unwrap();
    assert_eq!(
        result,
        vec![AuthoringServiceEntry {
            id: layer2_core::ServiceId::new("research"),
            catalog_id: Some("service-uuid".into()),
            name: "Research".into(),
            description: Some("Read papers".into())
        }]
    );
    assert_eq!(*tokens.owners.lock().unwrap(), vec![owner()]);
    assert_eq!(
        upstream.seen()[0].authorization.as_deref(),
        Some("Bearer catalogue-owner-authority")
    );
}

#[tokio::test]
async fn missing_owner_credentials_never_fall_back_to_anonymous_catalogues() {
    let upstream = Upstream::start().await;
    let source = upstream.source(Some(TestTokens::new(None)));
    assert!(matches!(
        source.services(&owner(), None).await,
        Err(AuthoringSourceError::Unavailable(_))
    ));
    assert!(matches!(
        source.skills(&owner(), &query(), None).await,
        Err(AuthoringSourceError::Unavailable(_))
    ));
    assert!(upstream.seen().is_empty());
    assert!(matches!(
        upstream.source(None).services(&owner(), None).await,
        Err(AuthoringSourceError::Unavailable(_))
    ));
}

#[tokio::test]
async fn catalogue_stream_bytes_and_rows_are_bounded_without_content_length() {
    let upstream = Upstream::start().await;
    let source = upstream.source(Some(TestTokens::new(Some("catalogue-reader"))));
    for value in [
        json!({"services":[{"id":"service-a","description":"x".repeat(http::MAX_RESPONSE_BYTES)}]}),
        json!({"services":vec![json!({"id":"service-a"});1001]}),
    ] {
        upstream.response("/api/v1/services", 200, value);
        assert!(matches!(
            source.services(&owner(), None).await,
            Err(AuthoringSourceError::Unavailable(_))
        ));
    }
}

#[tokio::test]
async fn skill_search_resolves_literal_versions_and_preserves_owner_scoped_pagination() {
    let upstream = Upstream::start().await;
    upstream.skills(
        json!([
            {"guid":"research","name":"Research","description":"Read papers"},
            {"guid":"nyxid","name":"Already installed"}
        ]),
        true,
        Some("next-page"),
    );
    upstream.versions(
        "research",
        json!([
            {"version":"1.9","isDeprecated":false},
            {"version":"2.0","isDeprecated":false},
            {"version":"3.0","isDeprecated":true}
        ]),
    );
    let source = upstream.source(Some(TestTokens::new(Some("skill-owner-authority"))));
    let result = source.skills(&owner(), &query(), None).await.unwrap();
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].id.as_str(), "research");
    assert_eq!(result.items[0].versions, ["2.0", "1.9"]);
    assert_eq!(result.next.as_deref(), Some("next-page"));
    let seen = upstream.seen();
    assert_eq!(seen.len(), 2);
    assert!(seen
        .iter()
        .all(|row| row.authorization.as_deref() == Some("Bearer skill-owner-authority")));
    let parsed = reqwest::Url::parse(&format!(
        "https://example.test/?{}",
        seen[0].query.as_ref().unwrap()
    ))
    .unwrap();
    let pairs: BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(pairs["q"], "research & tools");
    assert_eq!(pairs["scope"], "mixed");
    assert_eq!(pairs["limit"], "20");
}

#[tokio::test]
async fn trusted_skill_search_is_explicitly_public_and_forwards_opaque_cursor_as_data() {
    let upstream = Upstream::start().await;
    upstream.skills(json!([]), false, None);
    let mut query = query();
    query.cursor = Some("opaque&scope=mine".into());
    upstream
        .source(None)
        .skills(&owner(), &query, None)
        .await
        .unwrap();
    let seen = upstream.seen();
    assert_eq!(seen[0].authorization, None);
    let parsed = reqwest::Url::parse(&format!(
        "https://example.test/?{}",
        seen[0].query.as_ref().unwrap()
    ))
    .unwrap();
    let pairs: BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(pairs["scope"], "public");
    assert_eq!(pairs["cursor"], "opaque&scope=mine");
}

#[tokio::test]
async fn skill_cursor_refusals_are_distinct_from_catalogue_outages() {
    let upstream = Upstream::start().await;
    let source = upstream.source(None);
    for status in [400, 401, 403, 404, 429, 500, 302] {
        upstream.response(
            &format!("{SKILLS}/skill-search"),
            status,
            json!({"error":"upstream detail"}),
        );
        let error = source.skills(&owner(), &query(), None).await.unwrap_err();
        if status == 400 {
            assert!(matches!(error, AuthoringSourceError::InvalidQuery(_)));
        } else {
            // The stage and status name the failing dependency (#2270).
            assert!(matches!(
                &error,
                AuthoringSourceError::Unavailable(detail)
                    if *detail == format!("skill search answered HTTP {status}")
            ));
        }
        assert!(!error.to_string().contains("upstream detail"));
    }
}

#[tokio::test]
async fn one_refused_version_read_names_its_stage_and_status() {
    let upstream = Upstream::start().await;
    let source = upstream.source(None);
    upstream.skills(json!([{"guid":"research","name":"Research"}]), false, None);
    upstream.response(
        &format!("{SKILLS}/skills/research/versions"),
        403,
        json!({"error":"upstream detail"}),
    );
    let error = source.skills(&owner(), &query(), None).await.unwrap_err();
    assert!(matches!(
        &error,
        AuthoringSourceError::Unavailable(detail) if detail == "skill versions answered HTTP 403"
    ));
    assert!(!error.to_string().contains("upstream detail"));
}

#[tokio::test]
async fn skill_search_rejects_nonadvancing_or_missing_cursors_and_excess_rows() {
    let upstream = Upstream::start().await;
    let source = upstream.source(None);
    let mut query = query();
    query.cursor = Some("current-page".into());
    for next in [None, Some(""), Some("current-page")] {
        upstream.skills(json!([]), true, next);
        assert!(matches!(
            source.skills(&owner(), &query, None).await,
            Err(AuthoringSourceError::InvalidResponse(_))
        ));
    }
    upstream.skills(
        json!(vec![json!({"guid":"research","name":"Research"}); 21]),
        false,
        None,
    );
    assert!(matches!(
        source.skills(&owner(), &query, None).await,
        Err(AuthoringSourceError::InvalidResponse(_))
    ));
}

#[tokio::test]
async fn skill_material_rejects_unsafe_ids_and_nonliteral_releases() {
    let upstream = Upstream::start().await;
    let source = upstream.source(None);
    upstream.skills(json!([{"guid":"../other","name":"Research"}]), false, None);
    assert!(matches!(
        source.skills(&owner(), &query(), None).await,
        Err(AuthoringSourceError::InvalidResponse(_))
    ));
    assert_eq!(upstream.seen().len(), 1);
    upstream.skills(json!([{"guid":"research","name":"Research"}]), false, None);
    for version in ["latest", "1.2.3", "01.0", "1.00", "1.", "1.-1"] {
        upstream.versions(
            "research",
            json!([{"version":version,"isDeprecated":false}]),
        );
        assert!(matches!(
            source.skills(&owner(), &query(), None).await,
            Err(AuthoringSourceError::InvalidResponse(_))
        ));
    }
}

#[tokio::test]
async fn malformed_and_oversized_skill_success_cannot_become_an_empty_catalogue() {
    let upstream = Upstream::start().await;
    let source = upstream.source(None);
    for value in [
        json!({}),
        json!({"data":{"items":[],"meta":{"hasMore":false}},"error":"failure"}),
        json!({"data":{"items":[{"guid":"research","name":"x".repeat(http::MAX_RESPONSE_BYTES)}]}}),
    ] {
        upstream.response(&format!("{SKILLS}/skill-search"), 200, value);
        assert!(matches!(
            source.skills(&owner(), &query(), None).await,
            Err(AuthoringSourceError::InvalidResponse(_))
        ));
    }
}

#[tokio::test]
async fn many_skill_releases_return_the_latest_bounded_selection_and_reject_duplicates() {
    let upstream = Upstream::start().await;
    upstream.skills(json!([{"guid":"research","name":"Research"}]), false, None);
    upstream.versions(
        "research",
        json!((0..150)
            .map(|minor| json!({"version":format!("1.{minor}"),"isDeprecated":false}))
            .collect::<Vec<_>>()),
    );
    let source = upstream.source(None);
    let result = source.skills(&owner(), &query(), None).await.unwrap();
    assert_eq!(result.items[0].versions.len(), 100);
    assert_eq!(result.items[0].versions[0], "1.149");
    assert_eq!(result.items[0].versions[99], "1.50");
    upstream.versions(
        "research",
        json!([
            {"version":"1.0","isDeprecated":false}, {"version":"1.0","isDeprecated":false}
        ]),
    );
    assert!(matches!(
        source.skills(&owner(), &query(), None).await,
        Err(AuthoringSourceError::InvalidResponse(_))
    ));
}

#[tokio::test]
async fn filtered_skills_preserve_the_continuation_to_reachable_results() {
    let upstream = Upstream::start().await;
    upstream.skills(
        json!([
            {"guid":"research","name":"Research"}, {"guid":"nyxid","name":"Installed"}
        ]),
        true,
        Some("next-page"),
    );
    upstream.versions("research", json!([{"version":"1.0","isDeprecated":true}]));
    let result = upstream
        .source(None)
        .skills(&owner(), &query(), None)
        .await
        .unwrap();
    assert!(result.items.is_empty());
    assert_eq!(result.next.as_deref(), Some("next-page"));
}

#[tokio::test]
async fn public_repository_default_branch_is_server_verified_without_owner_credentials() {
    let upstream = Upstream::start().await;
    upstream.repository();
    upstream.reference("heads", "main", "commit", SHA);
    let tokens = TestTokens::new(Some("never-send-to-github"));
    let source = upstream.source(Some(tokens.clone()));
    let mut request = repository(None);
    request.url.push_str(".git");
    let result = source.resolve_repository(&request).await.unwrap();
    assert_eq!(result.url, repository(None).url);
    assert_eq!(result.reference, None);
    assert_eq!(result.default_branch, "main");
    assert_eq!(result.commit_sha, SHA);
    assert!(tokens.owners.lock().unwrap().is_empty());
    assert_eq!(upstream.seen().len(), 2);
    assert!(upstream
        .seen()
        .iter()
        .all(|row| row.authorization.is_none()));
}

#[tokio::test]
async fn repository_branch_with_slash_is_encoded_and_matched_exactly() {
    let upstream = Upstream::start().await;
    upstream.repository();
    upstream.reference("heads", "feature/research", "commit", SHA);
    let result = upstream
        .source(None)
        .resolve_repository(&repository(Some("feature/research")))
        .await
        .unwrap();
    assert_eq!(result.reference.as_deref(), Some("feature/research"));
    assert!(upstream.seen()[1]
        .path
        .ends_with("/heads/feature%2Fresearch"));
}

#[tokio::test]
async fn annotated_tag_is_peeled_from_the_observed_exact_object() {
    let upstream = Upstream::start().await;
    upstream.repository();
    upstream.reference("tags", "v1.0", "tag", TAG_SHA);
    upstream.response(
        &format!("/repos/the-omega-institute/trureturing/git/tags/{TAG_SHA}"),
        200,
        json!({"sha":TAG_SHA,"object":{"type":"commit","sha":SHA}}),
    );
    let result = upstream
        .source(None)
        .resolve_repository(&repository(Some("v1.0")))
        .await
        .unwrap();
    assert_eq!(result.commit_sha, SHA);
    assert_eq!(result.reference.as_deref(), Some("v1.0"));
    assert_eq!(upstream.seen().len(), 4);
}

#[tokio::test]
async fn missing_repository_and_reference_are_distinct_from_rate_limits_and_redirects() {
    let upstream = Upstream::start().await;
    let source = upstream.source(None);
    assert!(matches!(
        source.resolve_repository(&repository(None)).await,
        Err(AuthoringSourceError::RepositoryNotFound)
    ));
    upstream.repository();
    assert!(matches!(
        source
            .resolve_repository(&repository(Some("missing")))
            .await,
        Err(AuthoringSourceError::ReferenceNotFound)
    ));
    for status in [301, 302, 403, 429, 500] {
        upstream.response(
            "/repos/the-omega-institute/trureturing",
            status,
            json!({"message":"upstream details"}),
        );
        assert!(matches!(
            source.resolve_repository(&repository(None)).await,
            Err(AuthoringSourceError::Unavailable(_))
        ));
    }
}

#[tokio::test]
async fn repository_tag_peeling_is_bounded_and_never_accepts_a_changed_object() {
    let upstream = Upstream::start().await;
    upstream.repository();
    upstream.reference("tags", "v1.0", "tag", TAG_SHA);
    let path = format!("/repos/the-omega-institute/trureturing/git/tags/{TAG_SHA}");
    upstream.response(
        &path,
        200,
        json!({"sha":TAG_SHA,"object":{"type":"tag","sha":TAG_SHA}}),
    );
    let source = upstream.source(None);
    assert!(matches!(
        source.resolve_repository(&repository(Some("v1.0"))).await,
        Err(AuthoringSourceError::InvalidResponse(_))
    ));
    assert_eq!(upstream.seen().len(), 11);
    upstream.response(
        &path,
        200,
        json!({"sha":SHA,"object":{"type":"commit","sha":SHA}}),
    );
    assert!(matches!(
        source.resolve_repository(&repository(Some("v1.0"))).await,
        Err(AuthoringSourceError::InvalidResponse(_))
    ));
}

#[tokio::test]
async fn repository_response_cannot_change_identity_or_admit_private_material() {
    let upstream = Upstream::start().await;
    let source = upstream.source(None);
    for (key, value, missing) in [
        ("private", json!(true), true),
        ("disabled", json!(true), true),
        ("full_name", json!("another/repository"), false),
        (
            "html_url",
            json!("https://attacker.example/repository"),
            false,
        ),
        ("default_branch", json!(""), false),
    ] {
        let mut response = json!({"full_name":"the-omega-institute/trureturing",
            "html_url":repository(None).url, "default_branch":"main","private":false,"disabled":false});
        response[key] = value;
        upstream.response("/repos/the-omega-institute/trureturing", 200, response);
        let result = source.resolve_repository(&repository(None)).await;
        if missing {
            assert!(matches!(
                result,
                Err(AuthoringSourceError::RepositoryNotFound)
            ));
        } else {
            assert!(matches!(
                result,
                Err(AuthoringSourceError::InvalidResponse(_))
            ));
        }
    }
    assert_eq!(upstream.seen().len(), 5);
}

#[tokio::test]
async fn repository_reference_identity_type_and_hash_are_checked() {
    let upstream = Upstream::start().await;
    upstream.repository();
    let source = upstream.source(None);
    for response in [
        json!({"ref":"refs/heads/other","object":{"type":"commit","sha":SHA}}),
        json!({"ref":"refs/heads/main","object":{"type":"tree","sha":SHA}}),
        json!({"ref":"refs/heads/main","object":{"type":"commit","sha":"not-an-object"}}),
    ] {
        upstream.response(
            "/repos/the-omega-institute/trureturing/git/ref/heads/main",
            200,
            response,
        );
        assert!(matches!(
            source.resolve_repository(&repository(None)).await,
            Err(AuthoringSourceError::InvalidResponse(_))
        ));
    }
}

#[tokio::test]
async fn repository_transport_refuses_non_github_or_encoded_path_locators_without_fetch() {
    let upstream = Upstream::start().await;
    let source = upstream.source(None);
    for url in [
        "http://github.com/owner/repo",
        "https://github.com.attacker.test/owner/repo",
        "https://user@github.com/owner/repo",
        "https://github.com/owner/repo?token=value",
        "https://github.com/owner/%2Frepo",
        "https://127.0.0.1/owner/repo",
        "https://github.com/owner/.git",
        "https://github.com/owner/..git",
        "https://github.com/owner/...git",
        "https://github.com/owner//repo",
        "https://github.com//owner/repo",
        "https://github.com/owner/repo//",
        "https://github.com/owner/./repo",
        "https://github.com/other/../owner/repo",
    ] {
        assert!(source
            .resolve_repository(&RepositoryRequest {
                url: url.into(),
                reference: None
            })
            .await
            .is_err());
    }
    assert!(upstream.seen().is_empty());
}

#[tokio::test]
async fn repository_name_bound_applies_after_the_optional_git_suffix() {
    let upstream = Upstream::start().await;
    let name = "r".repeat(100);
    let canonical = format!("https://github.com/owner/{name}");
    upstream.response(
        &format!("/repos/owner/{name}"),
        200,
        json!({
            "full_name":format!("owner/{name}"),"html_url":canonical,
            "default_branch":"main","private":false,"disabled":false
        }),
    );
    upstream.response(
        &format!("/repos/owner/{name}/git/ref/heads/main"),
        200,
        json!({
            "ref":"refs/heads/main","object":{"type":"commit","sha":SHA}
        }),
    );
    let source = upstream.source(None);
    let result = source
        .resolve_repository(&RepositoryRequest {
            url: format!("{canonical}.git/"),
            reference: None,
        })
        .await
        .unwrap();
    assert_eq!(result.url, canonical);
    assert_eq!(upstream.seen().len(), 2);
    assert!(source
        .resolve_repository(&RepositoryRequest {
            url: format!("{canonical}x.git"),
            reference: None,
        })
        .await
        .is_err());
    assert_eq!(upstream.seen().len(), 2);
}

#[tokio::test]
#[ignore = "reads the public GitHub API; run explicitly when network verification is available"]
async fn live_trureturing_repository_resolution() {
    let resolver = repositories::RepositoryResolver {
        http: http::client().unwrap(),
        origin: reqwest::Url::parse("https://api.github.com").unwrap(),
    };
    let result = resolver.resolve(&repository(None)).await.unwrap();
    assert_eq!(result.url, repository(None).url);
    assert!(!result.default_branch.is_empty());
    assert_eq!(result.commit_sha.len(), 40);
}
