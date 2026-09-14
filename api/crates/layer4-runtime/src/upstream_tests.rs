use layer3_adapter::{CredentialFrame, RuntimeCredential};

use super::*;

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
        })
        .await
        .unwrap();
    let rollback = holder
        .install(&CredentialFrame {
            operation_id: "two".into(),
            value: RuntimeCredential::new("secret-two"),
            custody_generation: Some(1),
        })
        .await
        .unwrap_err();
    assert_eq!(rollback.code, "credential_generation_rollback");
    let conflict = holder
        .install(&CredentialFrame {
            operation_id: "three".into(),
            value: RuntimeCredential::new("secret-three"),
            custody_generation: Some(2),
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
        })
        .await
        .unwrap();
    let state = ProxyState {
        target: TargetBase::parse(&origin).unwrap(),
        credential,
        client: reqwest::Client::new(),
        allowed_host: None,
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
