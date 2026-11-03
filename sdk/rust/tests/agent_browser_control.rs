#[path = "support/wire.rs"]
mod wire;
use cma_sdk::Error;
use wire::{Expected, WireServer, headers};

#[tokio::test]
async fn browser_control_preserves_operation_identity_and_rejects_a_foreign_context_receipt() {
    let server = WireServer::new().await;
    let input = serde_json::json!({"runtime_incarnation":"current", "context_id":"e9ed9ab9-02a9-4c1e-91f5-7e13ca191b24",
        "client_id":"client-one", "operation_id":"78af4405-b644-44ab-b58d-53ce4d62acc0", "command":"acquire", "expected_revision":0});
    let body = serde_json::json!({"agent_id":"agent", "runtime_incarnation":"current", "context_id":input["context_id"],
        "state":"user", "revision":1, "client_id":"client-one", "lease_token":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        "expires_at":"2026-11-03T15:00:15Z", "lease_duration_ms":15000, "operation_id":input["operation_id"],
        "operation_revision":1, "audit":[], "audit_truncated_before":null});
    for context in [input["context_id"].clone(), serde_json::json!("foreign")] {
        let mut response = body.clone();
        response["context_id"] = context;
        let mut expected_headers = headers(true, "application/json");
        expected_headers.insert("content-type".into(), "application/json".into());
        expected_headers.insert(
            "x-cma-browser-client".into(),
            "b2f66e8e-5ddd-4e96-b2e9-84eb55195664".into(),
        );
        server.expect(Expected {
            method: "POST".into(),
            path: "/api/v1/agents/agent/browser/control".into(),
            headers: expected_headers,
            body: Some(input.clone()),
            status: 200,
            response: serde_json::to_vec(&response).unwrap(),
            ..Default::default()
        });
    }
    let mut command: cma_sdk::agent_browser_control::BrowserControlChange =
        serde_json::from_value(input).unwrap();
    command.scope.client_key = "b2f66e8e-5ddd-4e96-b2e9-84eb55195664".into();
    assert!(!format!("{command:?}").contains(&command.scope.client_key));
    let receipt = server
        .client
        .agent_browser_control()
        .change("agent", &command)
        .await
        .unwrap();
    assert_eq!(receipt.state.as_str(), "user");
    assert_eq!(receipt.operation_revision, Some(1));
    assert!(matches!(
        server
            .client
            .agent_browser_control()
            .change("agent", &command)
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}
