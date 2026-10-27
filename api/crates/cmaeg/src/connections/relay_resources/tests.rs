use super::*;
use axum::{
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Mutex;

fn inventory(platform: ResourcePlatform) -> (Value, Value, Value) {
    let (slug, origin) = platform.service();
    (
        json!({"services":[{"id":"owned-connection","slug":slug,"label":"Research bot","endpoint_id":"endpoint","catalog_service_id":"catalog","is_active":true,"auth_method":"token_exchange","credential_source":{"type":"personal"},"private_value":"must-not-project"}]}),
        json!({"endpoints":[{"id":"endpoint","catalog_service_id":"catalog","url":origin}]}),
        json!({"services":[{"id":"catalog","slug":slug,"name":"Bot API","base_url":origin,"auth_method":"token_exchange","is_active":true,"private_value":"must-not-project"}]}),
    )
}

#[test]
fn discovery_preserves_the_existing_trusted_deployment_origin_policy() {
    let auth = Arc::new(CreatorAuthenticator::new("https://nyxid.example").unwrap());
    for origin in [
        "https://nyxid.example",
        "http://127.0.0.1:8080",
        "http://nyxid.chrono-bot.svc",
    ] {
        assert!(ResourceConnections::new(auth.clone(), origin).is_ok());
    }
    for origin in [
        "http://public.example",
        "https://nyxid.example/untrusted",
        "https://user@nyxid.example",
        "https://nyxid.example?next=other",
    ] {
        assert!(ResourceConnections::new(auth.clone(), origin).is_err());
    }
}
fn project_values(
    values: (Value, Value, Value),
    platform: ResourcePlatform,
) -> Result<ResourceConnectionList, DiscoveryError> {
    project(
        serde_json::from_value(values.0).unwrap(),
        serde_json::from_value(values.1).unwrap(),
        serde_json::from_value(values.2).unwrap(),
        platform,
    )
}

#[test]
fn discovers_exact_regional_personal_connections_and_projects_no_extra_metadata() {
    for platform in [ResourcePlatform::Lark, ResourcePlatform::Feishu] {
        let values = inventory(platform);
        let list = project_values(values.clone(), platform).unwrap();
        assert_eq!(
            serde_json::to_value(&list).unwrap(),
            json!({"items":[{"user_service_id":"owned-connection","catalog_service_id":"catalog","name":"Research bot","platform":platform,"source":"personal"}]})
        );
        let other = if platform == ResourcePlatform::Lark {
            ResourcePlatform::Feishu
        } else {
            ResourcePlatform::Lark
        };
        assert!(project_values(values, other).unwrap().items.is_empty());
    }
}

#[test]
fn excludes_disallowed_shared_connections_and_near_matching_metadata() {
    for (document, pointer, replacement) in [
        (
            0,
            "/services/0/credential_source",
            json!({"type":"org","org_id":"shared","allowed":false,"role":"viewer"}),
        ),
        (0, "/services/0/credential_source", Value::Null),
        (0, "/services/0/is_active", json!(false)),
        (0, "/services/0/auth_method", json!("bearer")),
        (0, "/services/0/node_id", json!("node")),
        (0, "/services/0/catalog_service_id", json!("other")),
        (0, "/services/0/endpoint_id", json!("missing")),
        (1, "/endpoints/0/catalog_service_id", json!("other")),
        (
            1,
            "/endpoints/0/url",
            json!("https://open.larksuite.com.evil.example"),
        ),
        (
            1,
            "/endpoints/0/url",
            json!("https://open.larksuite.com/extra"),
        ),
        (2, "/services/0/base_url", json!("https://open.feishu.cn")),
        (2, "/services/0/slug", json!("api-feishu-bot")),
        (2, "/services/0/auth_method", json!("bearer")),
        (2, "/services/0/is_active", json!(false)),
    ] {
        let (mut a, mut b, mut c) = inventory(ResourcePlatform::Lark);
        let value = match document {
            0 => &mut a,
            1 => &mut b,
            _ => &mut c,
        };
        if pointer.ends_with("/node_id") {
            value["services"][0]["node_id"] = replacement;
        } else {
            *value.pointer_mut(pointer).unwrap() = replacement;
        }
        assert!(
            project_values((a, b, c), ResourcePlatform::Lark)
                .unwrap()
                .items
                .is_empty(),
            "{pointer}"
        );
    }
}

#[test]
fn rejects_ambiguous_and_oversized_inventories_and_accepts_only_official_hidden_endpoints() {
    let (a, mut b, c) = inventory(ResourcePlatform::Lark);
    b["endpoints"][0]["url"] = Value::Null;
    b["endpoints"][0]["auto_connected"] = json!(true);
    assert_eq!(
        project_values((a.clone(), b.clone(), c.clone()), ResourcePlatform::Lark)
            .unwrap()
            .items
            .len(),
        1
    );
    b["endpoints"][0]["auto_connected"] = json!(false);
    assert!(project_values((a.clone(), b, c), ResourcePlatform::Lark)
        .unwrap()
        .items
        .is_empty());
    for document in 0..3 {
        let (mut a, mut b, mut c) = inventory(ResourcePlatform::Lark);
        let (value, field) = match document {
            0 => (&mut a, "services"),
            1 => (&mut b, "endpoints"),
            _ => (&mut c, "services"),
        };
        let duplicate = value[field][0].clone();
        value[field].as_array_mut().unwrap().push(duplicate);
        assert!(project_values((a, b, c), ResourcePlatform::Lark).is_err());
    }
    let (_, b, c) = inventory(ResourcePlatform::Lark);
    let row = a["services"][0].clone();
    assert!(project_values(
        (json!({"services":vec![row;MAX_ROWS+1]}), b, c),
        ResourcePlatform::Lark
    )
    .is_err());
}

#[derive(Clone)]
struct FixtureState {
    data: (Value, Value, Value),
    calls: Arc<Mutex<Vec<(String, String)>>>,
    status: StatusCode,
    active: bool,
    permission: bool,
    oversized: bool,
    authorization: Option<(StatusCode, Value)>,
}
struct Fixture {
    client: reqwest::Client,
    origin: String,
    calls: Arc<Mutex<Vec<(String, String)>>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    (
        origin,
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    )
}
impl Fixture {
    async fn new(status: StatusCode, active: bool, permission: bool, oversized: bool) -> Self {
        Self::with_data(
            status,
            active,
            permission,
            oversized,
            inventory(ResourcePlatform::Lark),
            None,
        )
        .await
    }
    async fn with_data(
        status: StatusCode,
        active: bool,
        permission: bool,
        oversized: bool,
        data: (Value, Value, Value),
        authorization: Option<(StatusCode, Value)>,
    ) -> Self {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let state = FixtureState {
            data,
            calls: calls.clone(),
            status,
            active,
            permission,
            oversized,
            authorization,
        };
        let issuer = Arc::new(Mutex::new(String::new()));
        let introspection = issuer.clone();
        let provider=Router::new()
            .route("/oauth/introspect",post(move |State(state):State<FixtureState>| {let issuer=introspection.clone();async move {Json(json!({"active":state.active,"sub":"owner","iss":*issuer.lock().unwrap(),"token_type":"access","exp":4102444800u64,"permissions":if state.permission {vec!["cma-eg:channels:read"]} else {vec![]}}))}}))
            .route("/api/v1/user-services",get(|State(state):State<FixtureState>, headers:HeaderMap| async move { read(state,headers,0) }))
            .route("/api/v1/endpoints",get(|State(state):State<FixtureState>, headers:HeaderMap| async move { read(state,headers,1) }))
            .route("/api/v1/services",get(|State(state):State<FixtureState>, headers:HeaderMap| async move { read(state,headers,2) }))
            .route("/api/v1/endpoints/endpoint/authorization",get(|State(state):State<FixtureState>, headers:HeaderMap| async move {
                state.calls.lock().unwrap().push(("3".into(), headers["authorization"].to_str().unwrap().into()));
                let (status, value) = state.authorization.unwrap_or((StatusCode::NOT_FOUND,json!({})));
                (status,Json(value))
            }))
            .with_state(state);
        let (base, provider_task) = serve(provider).await;
        *issuer.lock().unwrap() = base.clone();
        let auth = crate::auth::CreatorAuthenticator::new(&base)
            .unwrap()
            .with_service(
                cma_service_auth::NyxIdAuth::new(
                    &base,
                    cma_service_auth::Service::EventGateway,
                    "inventory-client",
                    "inventory-client-fixture".into(),
                )
                .unwrap(),
            );
        let discovery = ResourceConnections::new(Arc::new(auth), &base).unwrap();
        let (origin, gateway_task) =
            serve(crate::http::relay_resource_connections::router(discovery)).await;
        Self {
            client: reqwest::Client::new(),
            origin,
            calls,
            tasks: vec![provider_task, gateway_task],
        }
    }
    fn get(&self, query: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!(
                "{}/v1/relay-resource-connections{query}",
                self.origin
            ))
            .bearer_auth("inventory-request-fixture")
    }
}
fn read(state: FixtureState, headers: HeaderMap, kind: usize) -> Response {
    state.calls.lock().unwrap().push((
        kind.to_string(),
        headers["authorization"].to_str().unwrap().to_owned(),
    ));
    if state.oversized {
        return "x".repeat(MAX_BYTES + 1).into_response();
    }
    if state.status.is_redirection() {
        return (state.status, [("location", "/must-not-follow")]).into_response();
    }
    let value = match kind {
        0 => state.data.0,
        1 => state.data.1,
        _ => state.data.2,
    };
    (state.status, Json(value)).into_response()
}

#[tokio::test]
async fn public_discovery_uses_exact_incoming_credential_for_three_read_only_metadata_calls() {
    let f = Fixture::new(StatusCode::OK, true, true, false).await;
    let response = f.get("?platform=lark").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.unwrap()["items"][0]["user_service_id"],
        "owned-connection"
    );
    let mut calls = f.calls.lock().unwrap().clone();
    calls.sort();
    assert_eq!(
        calls,
        vec![
            ("0".into(), "Bearer inventory-request-fixture".into()),
            ("1".into(), "Bearer inventory-request-fixture".into()),
            ("2".into(), "Bearer inventory-request-fixture".into())
        ]
    );
}

#[tokio::test]
async fn auth_region_and_owner_refusals_never_read_inventory() {
    for (active, permission, status) in [
        (false, true, StatusCode::UNAUTHORIZED),
        (true, false, StatusCode::FORBIDDEN),
    ] {
        let f = Fixture::new(StatusCode::OK, active, permission, false).await;
        assert_eq!(
            f.get("?platform=lark").send().await.unwrap().status(),
            status
        );
        assert!(f.calls.lock().unwrap().is_empty());
    }
    let f = Fixture::new(StatusCode::OK, true, true, false).await;
    for query in ["", "?platform=telegram", "?platform=lark&owner=other"] {
        assert_eq!(
            f.get(query).send().await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        f.get("?platform=lark")
            .header("x-cmaeg-owner", "other")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.client
            .get(format!(
                "{}/v1/relay-resource-connections?platform=lark",
                f.origin
            ))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(f.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn upstream_refusals_outages_redirects_and_byte_caps_are_not_successful_empty_inventories() {
    for (upstream, oversized, expected) in [
        (StatusCode::UNAUTHORIZED, false, StatusCode::UNAUTHORIZED),
        (StatusCode::FORBIDDEN, false, StatusCode::FORBIDDEN),
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            false,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (StatusCode::FOUND, false, StatusCode::SERVICE_UNAVAILABLE),
        (StatusCode::OK, true, StatusCode::SERVICE_UNAVAILABLE),
    ] {
        let f = Fixture::new(upstream, true, true, oversized).await;
        let response = f.get("?platform=lark").send().await.unwrap();
        assert_eq!(response.status(), expected);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let value = response.json::<Value>().await.unwrap();
        assert!(value.get("items").is_none());
        assert!(!value.to_string().contains("inventory-request-fixture"));
    }
}

#[test]
fn code_generated_openapi_exposes_only_platform_and_creator_authority() {
    let value: Value = serde_json::from_str(&crate::openapi::snapshot()).unwrap();
    let operation = &value["paths"]["/relay-resource-connections"]["get"];
    assert_eq!(operation["operationId"], "listRelayResourceConnections");
    assert_eq!(operation["parameters"].as_array().unwrap().len(), 1);
    assert_eq!(operation["parameters"][0]["name"], "platform");
    assert_eq!(operation["parameters"][0]["in"], "query");
    assert_eq!(operation["security"], json!([{"creatorBearer":[]}]))
}

#[path = "shared_tests.rs"]
mod shared;
