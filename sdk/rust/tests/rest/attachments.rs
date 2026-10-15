use super::*;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    routing::post,
};

#[tokio::test]
async fn attachments_send_binary_images_with_owner_credentials_and_stable_key() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route("/api/v1/agents/agent-1/attachments", post(|request: Request<Body>| async move {
        assert_eq!(request.uri().query(), Some("name=review+file.png"));
        assert_eq!(request.headers()["content-type"], "image/png");
        assert_eq!(request.headers()["authorization"], "Bearer sdk-fixture");
        assert_eq!(request.headers()["idempotency-key"], "command-819");
        assert_eq!(to_bytes(request.into_body(), 1024).await.unwrap(), [0,255,10].as_slice());
        let id = format!("att_{}", "a".repeat(64));
        (StatusCode::CREATED, axum::Json(json!({"id":id,"agent_id":"agent-1","name":"review_file.png","path":format!("/home/attachments/agent-1/{id}/review_file.png"),"size_bytes":3,"media_type":"image/png","sha256":"c".repeat(64)})))
    }));
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = cma_sdk::Client::builder(format!("http://{address}"))
        .unwrap()
        .bearer_token("sdk-fixture")
        .unwrap()
        .build()
        .unwrap();
    let receipt = client
        .agents()
        .upload_attachment(
            "agent-1",
            "review file.png",
            "image/png",
            vec![0, 255, 10],
            &key(),
        )
        .await
        .unwrap();
    assert_eq!(receipt.name, "review_file.png");
    assert_eq!(receipt.size_bytes, 3);
    task.abort();
}

#[tokio::test]
async fn attachments_refuse_dangerous_names_and_size_before_transport() {
    let server = Server::new().await;
    for name in ["../file", "a/b", "a\\b", "..", ""] {
        assert!(matches!(
            server
                .client
                .agents()
                .upload_attachment("agent-1", name, "text/plain", vec![], &key())
                .await,
            Err(Error::Validation { .. })
        ));
    }
    assert!(matches!(
        server
            .client
            .agents()
            .upload_attachment(
                "agent-1",
                "large",
                "application/octet-stream",
                vec![0; 20 * 1024 * 1024 + 1],
                &key()
            )
            .await,
        Err(Error::Validation { .. })
    ));
    server.assert_drained();
}
