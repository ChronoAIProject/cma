#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{IdempotencyKey, agents::AgentLifecycleCreateRequest};
use serde_json::json;
use wire::{WireServer, json as response};

#[tokio::test]
async fn lifecycle_replays_exact_task_bytes_and_keeps_legacy_omission() {
    let server = WireServer::new().await;
    let key = IdempotencyKey::new("original-task-key").unwrap();
    let request = AgentLifecycleCreateRequest {
        name: "New task".into(),
        initial_message: Some("  比较三个工具\n保留原文。  ".into()),
        workspace_id: None,
        sandbox_id: None,
        image: None,
        repository: None,
        profile: None,
    };
    for _attempt in 0..2 {
        let mut expected = response(
            "POST",
            "/api/v1/agents",
            Some(json!({"name":"New task","initial_message":"  比较三个工具\n保留原文。  "})),
            202,
            json!({"id":"operation-original","status":"provisioning"}),
            true,
        );
        expected
            .headers
            .insert("idempotency-key".into(), "original-task-key".into());
        server.expect(expected);
        let receipt = server
            .client
            .agents()
            .create_lifecycle(&request, &key)
            .await
            .unwrap();
        assert_eq!(receipt.id, "operation-original");
    }
    let legacy: AgentLifecycleCreateRequest =
        serde_json::from_value(json!({"name":"Legacy"})).unwrap();
    let mut expected = response(
        "POST",
        "/api/v1/agents",
        Some(json!({"name":"Legacy"})),
        202,
        json!({"id":"legacy-operation","status":"provisioning"}),
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), "legacy-key".into());
    server.expect(expected);
    server
        .client
        .agents()
        .create_lifecycle(&legacy, &IdempotencyKey::new("legacy-key").unwrap())
        .await
        .unwrap();
    server.assert_drained();
    assert_eq!(server.request_count(), 3);
    let _ = (&server.anonymous, &server.base_url);
}
