use layer3_adapter::{CredentialFrame, RuntimeCredential};

use super::*;
use axum::http::HeaderMap;

#[tokio::test]
async fn model_credit_required_refuses_before_the_upstream_write() {
    use tower::ServiceExt;

    let destination = TargetBase::parse("http://127.0.0.1:9").unwrap();
    let credential = CredentialHolder::new();
    credential
        .install(&CredentialFrame {
            operation_id: "funding-fixture".into(),
            value: RuntimeCredential::new("fixture-model-credential"),
            custody_generation: Some(1),
            github_route: None,
        })
        .await
        .unwrap();
    let response = proxy_router(destination, credential, None, true, true)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/responses")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 128)
            .await
            .unwrap(),
        "model_funding_unsupported"
    );
}

#[test]
fn restricted_proxy_refuses_ambiguous_hosts_and_absolute_destinations() {
    for (uri, host, method, allowed) in [
        ("/user", "api.github.com", "GET", true),
        ("/user", "API.GITHUB.COM:443", "GET", true),
        ("https://api.github.com/user", "api.github.com", "GET", true),
        (
            "https://untrusted.example/user",
            "api.github.com",
            "GET",
            false,
        ),
        ("http://api.github.com/user", "api.github.com", "GET", false),
        ("/user", "api.github.com:80", "GET", false),
        ("/user", "api.github.com.untrusted.example", "GET", false),
        ("api.github.com:443", "api.github.com", "CONNECT", false),
    ] {
        let request = Request::builder()
            .uri(uri)
            .method(method)
            .header(header::HOST, host)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            allowed_request_host(&request, "api.github.com"),
            allowed,
            "{method} {uri} {host}"
        );
    }
    let mut request = Request::builder().uri("/user").body(Body::empty()).unwrap();
    assert!(!allowed_request_host(&request, "api.github.com"));
    request
        .headers_mut()
        .append(header::HOST, HeaderValue::from_static("api.github.com"));
    request
        .headers_mut()
        .append(header::HOST, HeaderValue::from_static("api.github.com"));
    assert!(!allowed_request_host(&request, "api.github.com"));
}

#[tokio::test]
async fn holder_refuses_generation_rollback_and_conflict() {
    let holder = CredentialHolder::new();
    holder
        .install(&CredentialFrame {
            operation_id: "one".into(),
            value: RuntimeCredential::new("secret-one"),
            custody_generation: Some(2),
            github_route: None,
        })
        .await
        .unwrap();
    let rollback = holder
        .install(&CredentialFrame {
            operation_id: "two".into(),
            value: RuntimeCredential::new("secret-two"),
            custody_generation: Some(1),
            github_route: None,
        })
        .await
        .unwrap_err();
    assert_eq!(rollback.code, "credential_generation_rollback");
    let conflict = holder
        .install(&CredentialFrame {
            operation_id: "three".into(),
            value: RuntimeCredential::new("secret-three"),
            custody_generation: Some(2),
            github_route: None,
        })
        .await
        .unwrap_err();
    assert_eq!(conflict.code, "credential_generation_conflict");
}

#[tokio::test]
async fn model_proxy_strips_provider_routing_authority_before_upstream() {
    use axum::routing::post;
    use tokio::sync::mpsc;

    let (captured, mut observed) = mpsc::channel(1);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let app = Router::new().route(
            "/v1/responses",
            post(move |headers: HeaderMap| {
                let captured = captured.clone();
                async move {
                    captured.send(headers).await.unwrap();
                    axum::Json(serde_json::json!({"output_text": "A Safe Title"}))
                }
            }),
        );
        axum::serve(listener, app).await.unwrap();
    });
    let credential = CredentialHolder::new();
    credential
        .install(&CredentialFrame {
            operation_id: "title-proxy-credential".into(),
            value: RuntimeCredential::new("current-scoped-title-key"),
            custody_generation: Some(3),
            github_route: None,
        })
        .await
        .unwrap();
    let state = ProxyState {
        target: TargetBase::parse(&origin).unwrap(),
        credential,
        client: reqwest::Client::new(),
        github_client: None,
        allowed_host: None,
        model_proxy: true,
        model_credit_required: false,
    };
    let request = Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("OPEN-SANDBOX-API-KEY", "private-provider-routing-key")
        .header(header::AUTHORIZATION, "Bearer caller-title-key")
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-request-id", "title-request")
        .body(Body::from("{}"))
        .unwrap();
    let response = forward(State(state), request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = observed.recv().await.unwrap();
    assert!(!headers.contains_key("open-sandbox-api-key"));
    assert_eq!(
        headers[header::AUTHORIZATION],
        "Bearer current-scoped-title-key"
    );
    assert_eq!(headers["x-request-id"], "title-request");
    assert!(!headers.values().any(|value| value
        .as_bytes()
        .windows(b"private-provider-routing-key".len())
        .any(|window| window == b"private-provider-routing-key")));
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn only_model_proxy_repairs_tool_free_compaction_and_keeps_credential_boundary() {
    use axum::body::Bytes;
    use axum::routing::post;
    use tokio::sync::mpsc;
    use tower::ServiceExt as _;

    let (captured, mut observed) = mpsc::channel(3);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/v1", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let app = Router::new().route(
            "/v1/responses",
            post(move |headers: HeaderMap, body: Bytes| {
                let captured = captured.clone();
                async move {
                    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let compatible = parsed["tools"] == serde_json::json!([
                        {"type":"web_search", "external_web_access":false}
                    ]) && parsed["tool_choice"] == "none";
                    captured.send((headers, parsed)).await.unwrap();
                    let event = if compatible {
                        "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n"
                    } else {
                        "event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"upstream_error\",\"message\":\"response protection is unavailable\"}}}\n\n"
                    };
                    ([(header::CONTENT_TYPE, "text/event-stream")], event)
                }
            }),
        );
        axum::serve(listener, app).await.unwrap();
    });
    let original = serde_json::json!({
        "model":"gpt-6", "tools":[], "tool_choice":"auto",
        "input":[{"type":"web_search_call","id":"retained-search","action":{"type":"search","query":"fixture"}}],
        "client_metadata":{"x-codex-turn-metadata":serde_json::json!({
            "request_kind":"compaction", "compaction":{"implementation":"responses","reason":"context_limit"}
        }).to_string()}
    });
    for role in ["model", "platform", "github"] {
        let credential = CredentialHolder::new();
        credential
            .install(&CredentialFrame {
                operation_id: "compaction-fixture-credential".into(),
                value: RuntimeCredential::new("fixture-scoped-credential"),
                custody_generation: Some(1),
                github_route: None,
            })
            .await
            .unwrap();
        let target = TargetBase::parse(&origin).unwrap();
        let proxy = match role {
            "model" => proxy_router(target, credential, None, true, false),
            "platform" => proxy_router(target, credential, None, false, false),
            _ => router(target, credential, Some("api.github.com")),
        };
        let body = serde_json::to_vec(&original).unwrap();
        let response = proxy
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/responses")
                    .header(header::HOST, "api.github.com")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::CONTENT_LENGTH, body.len())
                    .header(header::AUTHORIZATION, "Bearer untrusted-caller-credential")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response_body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let response_body = std::str::from_utf8(&response_body).unwrap();
        let (headers, received) = observed.recv().await.unwrap();
        assert_eq!(
            headers[header::AUTHORIZATION],
            "Bearer fixture-scoped-credential"
        );
        assert_eq!(received["input"], original["input"]);
        assert_eq!(received["client_metadata"], original["client_metadata"]);
        if role == "model" {
            assert!(response_body.contains("response.completed"));
            assert_eq!(received["tool_choice"], "none");
        } else {
            assert!(response_body.contains("response protection is unavailable"));
            assert_eq!(received, original);
        }
    }
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn selected_github_write_preserves_the_provider_scope_decision() {
    use axum::{extract::OriginalUri, routing::post};
    use layer3_adapter::{GithubCredentialRoute, GithubRepositoryScope, GithubRepositoryScopeNeed};
    use tower::ServiceExt as _;

    const SELECTED: &str = "05ceabf7-ad92-42d0-9be7-047aa3f869e7";
    const REPAIR: &str = "https://console.example.test/keys/05ceabf7-ad92-42d0-9be7-047aa3f869e7";
    const INDEPENDENT: &str = r#"{"message":"Forbidden","errors":[{"code":"branch_protection"}]}"#;
    const RATE_LIMIT: &str = r#"{"message":"You have exceeded a secondary rate limit"}"#;
    const SSO: &str = r#"{"message":"Forbidden"}"#;

    let typed_refusal = serde_json::json!({
        "code": "insufficient_scope",
        "status": 403,
        "detail": "public_repo",
        "message": "GitHub connection requires public_repo for this repository write",
        "connection_id": SELECTED,
        "remediation_url": REPAIR,
    })
    .to_string();
    let provider_typed_refusal = typed_refusal.clone();
    let provider = Router::new().route(
        "/api/v1/proxy/catalog/repos/example/project/git/refs",
        post(move |OriginalUri(uri): OriginalUri, headers: HeaderMap| {
            let typed_refusal = provider_typed_refusal.clone();
            async move {
                assert_eq!(headers[header::AUTHORIZATION], "Bearer proxy-fixture");
                let query: Vec<_> =
                    url::form_urlencoded::parse(uri.query().unwrap().as_bytes()).collect();
                assert_eq!(query.len(), 2);
                assert_eq!(query[1].0, "_nyxid_via");
                assert_eq!(query[1].1, SELECTED);
                let (body, content_type) = match query[0].1.as_ref() {
                    "scope" => (typed_refusal, "application/problem+json"),
                    "branch" => (INDEPENDENT.into(), "application/json"),
                    "rate" => (RATE_LIMIT.into(), "application/json"),
                    "sso" => (SSO.into(), "application/json"),
                    other => panic!("unexpected fixture {other}"),
                };
                Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .header(header::CONTENT_TYPE, content_type)
                    .header("x-oauth-scopes", "read:user")
                    .header("x-accepted-oauth-scopes", "public_repo, repo")
                    .header("x-github-request-id", "fixture")
                    .header("x-ratelimit-remaining", "97")
                    .body(Body::from(body))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = TargetBase::parse(&format!(
        "http://{}/api/v1/proxy/s/api-github",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
    let credential = CredentialHolder::new();
    credential
        .install(&CredentialFrame {
            operation_id: "github-fixture".into(),
            value: RuntimeCredential::new("proxy-fixture"),
            custody_generation: Some(1),
            github_route: Some(GithubCredentialRoute {
                service_id: SELECTED.into(),
                catalog_id: "catalog".into(),
                other_services: vec![],
                repository_scope: Some(GithubRepositoryScope {
                    repository: "example/project".into(),
                    required_scope: GithubRepositoryScopeNeed::PublicRepo,
                    manage_url: REPAIR.into(),
                }),
            }),
        })
        .await
        .unwrap();
    let proxy = router(target, credential, Some("api.github.com"));
    for (case, expected, content_type) in [
        ("scope", typed_refusal.as_str(), "application/problem+json"),
        ("branch", INDEPENDENT, "application/json"),
        ("rate", RATE_LIMIT, "application/json"),
        ("sso", SSO, "application/json"),
    ] {
        let response = proxy
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/repos/example/project/git/refs?case={case}"))
                    .header(header::HOST, "api.github.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
        assert_eq!(response.headers()["x-oauth-scopes"], "read:user");
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap(),
            expected
        );
    }
    server.abort();
    let _ = server.await;
}
