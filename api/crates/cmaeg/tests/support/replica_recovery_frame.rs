//! Obtain an authenticated replay frame through the public provider reader;
//! tests cannot manufacture its private trace/validation fields.
use axum::{routing::get, Router};
use cmaeg::providers::{ProviderClient, ProviderRegistry, RecoveryEvent, RecoveryIdentity};
use cmaeg_core::{channel::Secret, ids::ProviderSlug};
use serde_json::json;
use std::time::Duration;

pub async fn frame(cursor: u64) -> RecoveryEvent {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let raw = json!({"type":"response.completed", "response_id":"response-fixture",
        "sequence_number":cursor,"response":{"id":"response-fixture","object":"response",
        "status":"completed","output":[],"conversation":{"id":"agent-fixture"},"metadata":{}}});
    let wire = format!("id: {cursor}\nevent: response.completed\ndata: {raw}\n\n");
    let router = Router::new().route(
        "/api/v2/responses/{response_id}/events",
        get(move || {
            let wire = wire.clone();
            async move {
                (
                    [
                        ("content-type", "text/event-stream"),
                        ("x-cma-response-id", "response-fixture"),
                    ],
                    wire,
                )
            }
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let registry = ProviderRegistry::parse(&json!([{
        "slug":"cma_codex","base_url":base,"kind":"responses_http","max_inflight":1,
        "paths":{"agent_card":"/.well-known/agent-card.json",
        "binding":"/api/v1/cmaeg/bindings/{binding_id}",
        "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}",
        "responses":"/api/v2/responses", "recovery":{
        "response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"}}
    }]).to_string()).unwrap();
    let provider =
        ProviderClient::new(registry, Duration::from_secs(2), Duration::from_secs(5), 1).unwrap();
    let mut stream = provider
        .replay_response(
            &ProviderSlug::new("cma_codex").unwrap(),
            &RecoveryIdentity {
                response_id: "response-fixture".into(),
                native_agent_id: Some("agent-fixture".into()),
                metadata: Default::default(),
            },
            cursor.checked_sub(1),
            &Secret::new("runtime-test-provider-bearer"),
        )
        .await
        .unwrap();
    let frame = stream.events.recv().await.unwrap().unwrap();
    assert!(stream.events.recv().await.is_none());
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    frame
}
