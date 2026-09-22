use axum::http::HeaderMap;
use layer3_adapter::{CredentialFrame, RuntimeCredential};
use tokio::{sync::mpsc, task::JoinHandle};

use super::*;

#[test]
fn endpoint_configuration_is_optional_and_cannot_redirect_credentials() {
    assert!(ToolProxyConfig::from_env(&BTreeMap::new())
        .unwrap()
        .is_none());
    for value in [
        "https://gateway.example/mcp",
        "http://127.0.0.1:8090/mcp",
        "http://[::1]:8090/mcp",
    ] {
        let env = BTreeMap::from([("CHRONO_TOOL_PROXY_URL".into(), value.into())]);
        assert_eq!(
            ToolProxyConfig::from_env(&env)
                .unwrap()
                .unwrap()
                .upstream
                .as_str(),
            value
        );
    }
    for value in [
        "http://gateway.example/mcp",
        "https://user@gateway.example/mcp",
        "https://gateway.example/mcp?token=value",
        "https://gateway.example/mcp#other",
        "https://gateway.example:0/mcp",
        "https://gateway.example/../mcp",
        "https://gateway.example/%252e%252e/mcp",
        " https://gateway.example/mcp",
    ] {
        let env = BTreeMap::from([("CHRONO_TOOL_PROXY_URL".into(), value.into())]);
        assert!(ToolProxyConfig::from_env(&env).is_err(), "accepted {value}");
    }
}

async fn serve(app: Router) -> (String, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (origin, task)
}

async fn install(holder: &CredentialHolder, generation: u64) {
    holder
        .install(&CredentialFrame {
            operation_id: format!("install-{generation}"),
            value: RuntimeCredential::new(format!("test-runtime-credential-{generation}")),
            custody_generation: Some(generation),
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn tool_calls_preserve_bytes_and_read_rotated_credentials_without_reopening() {
    let (sent, mut observed) = mpsc::channel(4);
    let (upstream, server) = serve(Router::new().route(
        "/mcp",
        post(move |headers: HeaderMap, body: axum::body::Bytes| {
            let sent = sent.clone();
            async move {
                sent.send((headers, body.clone())).await.unwrap();
                ([(header::CONTENT_TYPE, "application/json")], body)
            }
        }),
    ))
    .await;
    let holder = CredentialHolder::new();
    let (local, proxy) = serve(router(
        ToolProxyConfig {
            upstream: format!("{upstream}/mcp").parse().unwrap(),
        },
        holder.clone(),
    ))
    .await;
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .post(&local)
            .body("{}")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let original = serde_json::to_vec(&serde_json::json!({
        "jsonrpc":"2.0", "id":7, "method":"tools/call",
        "params":{"name":"readEventContext","arguments":{"text":"x".repeat(32 * 1024)}}
    }))
    .unwrap();
    for generation in [1, 2] {
        install(&holder, generation).await;
        let response = client
            .post(&local)
            .header(header::AUTHORIZATION, "Bearer untrusted-caller-placeholder")
            .header("open-sandbox-api-key", "untrusted-routing-placeholder")
            .header(header::ORIGIN, "https://local.example")
            .header("mcp-protocol-version", "2025-06-18")
            .header(header::CONTENT_TYPE, "application/json")
            .body(original.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.bytes().await.unwrap(), original);
        let (headers, body) = observed.recv().await.unwrap();
        assert_eq!(
            headers[header::AUTHORIZATION],
            format!("Bearer test-runtime-credential-{generation}")
        );
        assert_eq!(headers["mcp-protocol-version"], "2025-06-18");
        assert!(!headers.contains_key("open-sandbox-api-key"));
        assert!(!headers.contains_key(header::ORIGIN));
        assert_eq!(body, original);
    }
    assert!(observed.try_recv().is_err());
    proxy.abort();
    server.abort();
}

#[tokio::test]
async fn full_context_replies_survive_json_escaping_and_dual_tool_result_encodings() {
    let activity = serde_json::json!({
        "kind":{"type":"message","text":"\u{0000}".repeat(32 * 1024),"attachments":[]},
        "actor":{"id":"sender","kind":"human"},
        "conversation":{"id":"conversation","kind":"private"},
        "occurred_at":"2026-09-22T00:00:00Z"
    });
    let result = serde_json::json!({
        "event_id":"event", "conversation_id":"conversation", "activity":activity,
        "capabilities":{"edit":false,"typing":false,"buttons":"numbered_text",
            "cards":"text","attachments":"link_text","max_text_chars":4000}
    });
    let expected = serde_json::to_vec(&serde_json::json!({
        "jsonrpc":"2.0","id":1,"result":{
            "content":[{"type":"text","text":result.to_string()}],
            "structuredContent":result,"isError":false
        }
    }))
    .unwrap();
    assert!(expected.len() > 256 * 1024);
    let reply = expected.clone();
    let (upstream, server) = serve(Router::new().route(
        "/mcp",
        post(move || {
            let reply = reply.clone();
            async move { ([(header::CONTENT_TYPE, "application/json")], reply) }
        }),
    ))
    .await;
    let holder = CredentialHolder::new();
    install(&holder, 1).await;
    let (local, proxy) = serve(router(
        ToolProxyConfig {
            upstream: format!("{upstream}/mcp").parse().unwrap(),
        },
        holder,
    ))
    .await;
    let response = reqwest::Client::new()
        .post(&local)
        .json(
            &serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"readEventContext","arguments":{"event_ref":"fixture-reference"}}}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.unwrap(), expected);
    proxy.abort();
    server.abort();
}

#[tokio::test]
async fn callers_cannot_select_another_endpoint_method_or_oversized_body() {
    let (sent, mut observed) = mpsc::channel(4);
    let (upstream, server) = serve(Router::new().fallback(move || {
        let sent = sent.clone();
        async move {
            sent.send(()).await.unwrap();
            "unexpected"
        }
    }))
    .await;
    let holder = CredentialHolder::new();
    install(&holder, 1).await;
    let (local, proxy) = serve(router(
        ToolProxyConfig {
            upstream: format!("{upstream}/mcp").parse().unwrap(),
        },
        holder,
    ))
    .await;
    let client = reqwest::Client::new();
    for (method, suffix, expected) in [
        (reqwest::Method::GET, "/", StatusCode::METHOD_NOT_ALLOWED),
        (reqwest::Method::DELETE, "/", StatusCode::METHOD_NOT_ALLOWED),
        (reqwest::Method::POST, "/v1/channels", StatusCode::NOT_FOUND),
        (
            reqwest::Method::POST,
            "/?destination=other",
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            client
                .request(method, format!("{local}{suffix}"))
                .body("{}")
                .send()
                .await
                .unwrap()
                .status(),
            expected
        );
    }
    assert_eq!(
        client
            .post(&local)
            .body(vec![b'x'; MAX_REQUEST_BYTES + 1])
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(observed.try_recv().is_err());
    proxy.abort();
    server.abort();
}

#[tokio::test]
async fn redirects_and_credential_echoes_are_refused_without_repeating_the_tool_call() {
    let (sent, mut observed) = mpsc::channel(8);
    let (upstream, server) = serve(Router::new().route(
        "/mcp",
        post(move |body: String| {
            let sent = sent.clone();
            async move {
                sent.send(body.clone()).await.unwrap();
                match body.as_str() {
                    "redirect" => (
                        StatusCode::TEMPORARY_REDIRECT,
                        [(header::LOCATION, "/different")],
                        "",
                    )
                        .into_response(),
                    "header" => ([("x-echo", "test-runtime-credential-1")], "{}").into_response(),
                    "body" => "test-runtime-credential-1".into_response(),
                    "large" => vec![b'x'; MAX_RESPONSE_BYTES + 1].into_response(),
                    _ => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
                }
            }
        }),
    ))
    .await;
    let holder = CredentialHolder::new();
    install(&holder, 1).await;
    let (local, proxy) = serve(router(
        ToolProxyConfig {
            upstream: format!("{upstream}/mcp").parse().unwrap(),
        },
        holder,
    ))
    .await;
    let client = reqwest::Client::new();
    for body in ["redirect", "header", "body", "large"] {
        let response = client.post(&local).body(body).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert!(!response
            .text()
            .await
            .unwrap()
            .contains("test-runtime-credential"));
        assert_eq!(observed.recv().await.unwrap(), body);
    }
    assert!(observed.try_recv().is_err());
    proxy.abort();
    server.abort();
}
