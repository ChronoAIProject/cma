use super::*;

fn artifact_id() -> String {
    format!("art_{}", "a".repeat(64))
}
fn metadata() -> Value {
    json!({"id":artifact_id(),"agent_id":"agent-1","name":"Review.txt","media_type":"text/plain","size_bytes":6,"sha256":"c".repeat(64),"created_at":NOW})
}
fn publication() -> PublishAgentArtifactRequest {
    PublishAgentArtifactRequest {
        name: "Review.txt".into(),
        media_type: "text/plain".into(),
        content_base64: "cmV2aWV3".into(),
    }
}
#[tokio::test]
async fn artifacts_publish_then_read_and_page_without_repeating_input() {
    let server = Server::new().await;
    let id = artifact_id();
    server.expect(
        "POST",
        "/agents/agent-1/artifacts",
        Some(json!({"name":"Review.txt","media_type":"text/plain","content_base64":"cmV2aWV3"})),
        201,
        Some(metadata()),
    );
    assert_eq!(
        server
            .client
            .agents()
            .publish_artifact("agent-1", &publication(), &key())
            .await
            .unwrap()
            .id,
        id
    );
    server.expect(
        "GET",
        &format!("/agents/agent-1/artifacts/{id}"),
        None,
        200,
        Some(metadata()),
    );
    assert_eq!(
        server
            .client
            .agents()
            .read_artifact("agent-1", &id)
            .await
            .unwrap()
            .agent_id,
        "agent-1"
    );
    server.expect(
        "GET",
        "/agents/agent-1/artifacts?limit=1",
        None,
        200,
        Some(json!({"items":[metadata()],"next":id})),
    );
    let options = PageOptions {
        limit: Some(1),
        cursor: None,
    };
    assert_eq!(
        server
            .client
            .agents()
            .list_artifacts("agent-1", &options)
            .await
            .unwrap()
            .next,
        Some(id.clone())
    );
    server.expect(
        "GET",
        &format!("/agents/agent-1/artifacts?cursor={id}&limit=1"),
        None,
        200,
        Some(json!({"items":[]})),
    );
    assert!(
        server
            .client
            .agents()
            .list_artifacts(
                "agent-1",
                &PageOptions {
                    cursor: Some(id),
                    ..options
                }
            )
            .await
            .unwrap()
            .items
            .is_empty()
    );
    server.assert_drained();
}
#[tokio::test]
async fn artifacts_refuse_changed_identity_malformed_metadata_and_unsafe_input() {
    let server = Server::new().await;
    let id = artifact_id();
    for (field, value) in [
        ("id", json!(format!("art_{}", "b".repeat(64)))),
        ("agent_id", json!("foreign")),
        ("size_bytes", json!(0)),
        ("size_bytes", json!(1048577)),
        ("name", json!("../private")),
        ("media_type", json!("text/html")),
        ("sha256", json!("no")),
    ] {
        let mut item = metadata();
        item[field] = value;
        server.expect(
            "GET",
            &format!("/agents/agent-1/artifacts/{id}"),
            None,
            200,
            Some(item),
        );
        assert!(matches!(
            server.client.agents().read_artifact("agent-1", &id).await,
            Err(Error::Protocol { .. })
        ));
    }
    for page in [
        json!({"items":[metadata(),metadata()]}),
        json!({"items":[],"next":id}),
        json!({"items":[metadata()],"next":"art_invalid"}),
    ] {
        server.expect("GET", "/agents/agent-1/artifacts", None, 200, Some(page));
        assert!(matches!(
            server
                .client
                .agents()
                .list_artifacts("agent-1", &PageOptions::default())
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    for content in ["", "YQ", "YR==", "YQ==\n"] {
        let mut body = publication();
        body.content_base64 = content.into();
        assert!(matches!(
            server
                .client
                .agents()
                .publish_artifact("agent-1", &body, &key())
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert!(matches!(
        server
            .client
            .agents()
            .list_artifacts(
                "agent-1",
                &PageOptions {
                    limit: Some(51),
                    cursor: None
                }
            )
            .await,
        Err(Error::Validation { .. })
    ));
    server.assert_drained();
}

#[tokio::test]
async fn artifact_download_requires_current_auth_and_opaque_bounded_content() {
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
        routing::get,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let route = format!(
        "/api/v1/agents/agent-1/artifacts/{}/download",
        artifact_id()
    );
    let router = Router::new().route(
        &route,
        get(|request: Request<Body>| async move {
            match request
                .headers()
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap()
            {
                "Bearer bytes" => (
                    StatusCode::OK,
                    [("content-type", "application/octet-stream")],
                    "review",
                ),
                "Bearer html" => (StatusCode::OK, [("content-type", "text/html")], "Sign in"),
                _ => (
                    StatusCode::NOT_FOUND,
                    [("content-type", "application/problem+json")],
                    r#"{"code":"resource_not_found","detail":"Access removed"}"#,
                ),
            }
        }),
    );
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for token in ["bytes", "html", "removed"] {
        let client = cma_sdk::Client::builder(format!("http://{address}"))
            .unwrap()
            .bearer_token(token)
            .unwrap()
            .build()
            .unwrap();
        let result = client
            .agents()
            .download_artifact("agent-1", &artifact_id())
            .await;
        match token {
            "bytes" => assert_eq!(result.unwrap(), b"review"),
            "html" => assert!(matches!(result, Err(Error::Protocol { .. }))),
            _ => assert!(matches!(result, Err(Error::Api { .. }))),
        }
    }
    task.abort();
}
