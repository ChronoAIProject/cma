use super::*;

#[tokio::test]
async fn authentication_accepts_only_one_exact_agent_bearer_header() {
    let stub = Stub::new(true);
    for case in 0..5 {
        let mut request = request(&stub, &call(proposal()));
        match case {
            0 => {
                request.headers_mut().remove(header::AUTHORIZATION);
            }
            1 => {
                request.headers_mut().insert(
                    header::AUTHORIZATION,
                    HeaderValue::from_static("Bearer nyx_key"),
                );
            }
            2 => {
                request.headers_mut().append(
                    header::AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {}", stub.bearer)).unwrap(),
                );
            }
            3 => {
                request.headers_mut().insert(
                    header::AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer cma_at_{}", "a".repeat(64))).unwrap(),
                );
            }
            _ => {
                request.headers_mut().remove(header::AUTHORIZATION);
                request.headers_mut().insert(
                    header::COOKIE,
                    HeaderValue::from_str(&format!("session={}", stub.bearer)).unwrap(),
                );
            }
        }
        let response = stub.router().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!body(response).await.to_string().contains(&stub.bearer));
    }
    assert!(stub.proposals.lock().unwrap().is_empty());
}

#[tokio::test]
async fn streamable_http_refuses_origin_bad_versions_large_bodies_and_other_methods() {
    let stub = Stub::new(true);
    for (case, expected) in [(0, 403), (1, 400), (2, 415), (3, 413)] {
        let mut request = request(&stub, &rpc("ping", json!({})));
        match case {
            0 => {
                request.headers_mut().insert(
                    header::ORIGIN,
                    HeaderValue::from_static("https://foreign.example"),
                );
            }
            1 => {
                request.headers_mut().insert(
                    "mcp-protocol-version",
                    HeaderValue::from_static("1900-01-01"),
                );
            }
            2 => {
                request
                    .headers_mut()
                    .insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
            }
            _ => {
                *request.body_mut() = Body::from("x".repeat(BODY_LIMIT + 1));
            }
        }
        assert_eq!(
            stub.router()
                .oneshot(request)
                .await
                .unwrap()
                .status()
                .as_u16(),
            expected
        );
    }
    for method in ["GET", "DELETE"] {
        let response = stub
            .router()
            .oneshot(
                HttpRequest::builder()
                    .method(method)
                    .uri("/api/v1/agent-tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    }
}

#[tokio::test]
async fn json_rpc_and_notifications_never_admit_an_unidentified_tool_call() {
    let stub = Stub::new(true);
    for value in [
        json!([]),
        json!({"jsonrpc":"1.0","id":7,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":{},"method":"ping"}),
        json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
    ] {
        assert_eq!(
            body(stub.router().oneshot(request(&stub, &value)).await.unwrap()).await["error"]
                ["code"],
            -32600
        );
    }
    let mut value = call(proposal());
    value.as_object_mut().unwrap().remove("id");
    assert_eq!(
        body(stub.router().oneshot(request(&stub, &value)).await.unwrap()).await["error"]["code"],
        -32600
    );
    assert!(stub.proposals.lock().unwrap().is_empty());
    assert_eq!(
        body(
            stub.router()
                .oneshot(request(&stub, &rpc("unknown", json!({}))))
                .await
                .unwrap()
        )
        .await["error"]["code"],
        -32601
    );
}
