#[path = "support/wire.rs"]
#[allow(dead_code)]
mod wire;
use cma_sdk::{Error, bridges::BridgeUsage};
use serde_json::{Value, json};
use wire::WireServer;

fn corpus() -> Value {
    serde_json::from_str(include_str!(
        "../../../api/crates/layer1-public/tests/fixtures/public-contract/bridge-usage.json"
    ))
    .unwrap()
}

#[tokio::test]
async fn usage_get_is_read_only_and_keeps_redaction_and_inventory_limits_explicit() {
    let server = WireServer::new().await;
    for case in corpus()["cases"].as_array().unwrap().iter().take(3) {
        let body = &case["response"]["body"];
        let bridge = body["bridge_id"].as_str().unwrap();
        let path = format!("/api/v1/bridges/{bridge}/usage");
        server.expect(wire::json("GET", &path, None, 200, body.clone(), true));
        assert_eq!(
            server.client.bridges().usage(bridge).await.unwrap(),
            serde_json::from_value::<BridgeUsage>(body.clone()).unwrap()
        );
    }
    server.assert_drained();
}

#[tokio::test]
async fn usage_rejects_inconsistent_counts_scope_redaction_and_unknown_fields() {
    let server = WireServer::new().await;
    let value = corpus()["cases"][0]["response"]["body"].clone();
    let bridge = value["bridge_id"].as_str().unwrap();
    let path = format!("/api/v1/bridges/{bridge}/usage");
    for (pointer, replacement) in [
        ("/bridge_id", json!("another")),
        ("/allocations", Value::Null),
        ("/inventory_limit", json!(255)),
        ("/observed_at_ms", json!(-1)),
        ("/allocations/current", json!(2)),
        ("/allocations/ready", json!(1)),
        ("/target_resources", json!({"status":"redacted"})),
        ("/target_resources/agents", json!({"retained":1,"active":2})),
        (
            "/limits",
            json!({"max_partitions":10,"max_sandbox_reservations":10,"credential":"private"}),
        ),
        ("/capacity_reasons", json!(["partition_limit_reached"])),
        (
            "/capacity_reasons",
            json!(["retired_sandbox_reservations_retained"]),
        ),
        (
            "/capacity_reasons",
            json!([
                "shared_parent_policy_applies",
                "shared_parent_policy_applies"
            ]),
        ),
    ] {
        let mut bad = value.clone();
        *bad.pointer_mut(pointer).unwrap() = replacement;
        server.expect(wire::json("GET", &path, None, 200, bad, true));
        assert!(
            matches!(
                server.client.bridges().usage(bridge).await,
                Err(Error::Protocol { .. })
            ),
            "{pointer}"
        );
    }
    server.assert_drained();
}
