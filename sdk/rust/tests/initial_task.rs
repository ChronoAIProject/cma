#[path = "support/wire.rs"]
mod wire;

use cma_sdk::{agents::AgentLifecycleCreateRequest, IdempotencyKey};
use serde_json::json;
use wire::{json as response, WireServer};

#[tokio::test]
async fn repository_task_reads_source_and_actual_checkout_as_distinct_public_facts() {
    let server = WireServer::new().await;
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../api/crates/layer1-public/tests/fixtures/public-contract/agents.json"
    ))
    .unwrap();
    let mut detail = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "agent-read")
        .unwrap()["response"]["body"]
        .clone();
    detail["workspace"]["resolved_commit"] = json!("b".repeat(40));
    detail["workspace"]["source"] = json!({"type":"repository_snapshot", "url":"https://github.com/example/project", "reference":"proof/fixed", "source_commit":"a".repeat(40)});
    server.expect(response(
        "GET",
        "/api/v1/agents/agt_contract",
        None,
        200,
        detail,
        true,
    ));
    let agent = server.client.agents().read("agt_contract").await.unwrap();
    assert_eq!(agent.workspace.resolved_commit, Some("b".repeat(40)));
    assert_eq!(
        agent.workspace.source,
        Some(cma_sdk::instances::WorkspaceSource::RepositorySnapshot {
            url: "https://github.com/example/project".into(),
            reference: Some("proof/fixed".into()),
            source_commit: Some("a".repeat(40)),
        })
    );
    server.assert_drained();
    let _ = (&server.anonymous, &server.base_url);
}

#[tokio::test]
async fn lifecycle_replays_exact_task_bytes_and_keeps_legacy_omission() {
    let server = WireServer::new().await;
    let key = IdempotencyKey::new("original-task-key").unwrap();
    let request = AgentLifecycleCreateRequest {
        name: "New task".into(),
        initial_message: Some("  比较三个工具\n保留原文。  ".into()),
        desktop: None,
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
