#[path = "support/wire.rs"]
mod wire;
use cma_sdk::Error;
use wire::{Expected, WireServer, headers};

#[tokio::test]
async fn browser_reads_are_bounded_and_bound_to_the_requested_agent_incarnation() {
    let server = WireServer::new().await;
    let body = serde_json::json!({"agent_id":"agent", "runtime_incarnation":"current", "state":"not_enabled",
        "context_id":null,"location":"Blank page","frame":null,"observers":0,"captures":0,"bytes":0,
        "min_interval_ms":1000,"fresh_for_ms":3000,"max_frame_bytes":262144,"max_observers":16});
    for incarnation in ["current", "old"] {
        let mut response = body.clone();
        response["runtime_incarnation"] = serde_json::json!(incarnation);
        server.expect(Expected {
            method: "GET".into(),
            path: "/api/v1/agents/agent/browser?runtime_incarnation=current&observer=reader".into(),
            headers: headers(false, "application/json"),
            status: 200,
            response: serde_json::to_vec(&response).unwrap(),
            ..Default::default()
        });
    }
    assert_eq!(
        server
            .anonymous
            .agent_browser()
            .read("agent", "current", Some("reader"), None)
            .await
            .unwrap()
            .state,
        "not_enabled"
    );
    assert!(matches!(
        server
            .anonymous
            .agent_browser()
            .read("agent", "current", Some("reader"), None)
            .await,
        Err(Error::Protocol { .. })
    ));
    server.assert_drained();
}
