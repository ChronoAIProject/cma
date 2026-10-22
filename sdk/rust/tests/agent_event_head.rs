#[path = "support/wire.rs"]
mod wire;
use cma_sdk::Error;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use wire::{Expected, WireServer};

fn cursor(sequence: u64) -> String {
    format!("ag1.{}.{}", "a".repeat(64), sequence)
}
fn head() -> Value {
    json!({
        "object":"cma.agent.event_head", "schema_version":1, "agent_id":"agt_fixture",
        "incarnation":"a".repeat(64), "through_cursor":cursor(9),
        "active_turn":{
            "cursor":cursor(7), "sequence":7, "appended_at":"2026-10-22T00:00:00Z",
            "kind":{"type":"turn_started","turn_sequence":7},
            "response_binding":{"response_id":"resp_fixture","command_id":"cmd_fixture","turn_sequence":7}
        }
    })
}
fn expected(value: &Value) -> Expected {
    Expected {
        method: "GET".into(),
        path: "/api/v2/agents/agt_fixture/events/head".into(),
        headers: wire::headers(true, "application/json"),
        status: 200,
        response_headers: BTreeMap::from([("content-type".into(), "application/json".into())]),
        response: serde_json::to_vec(value).unwrap(),
        ..Default::default()
    }
}

#[tokio::test]
async fn reads_current_bound_turn_or_idle_without_downloading_history() {
    let server = WireServer::new().await;
    let _ = (&server.anonymous, &server.base_url);
    server.expect(expected(&head()));
    let value = server
        .client
        .agent_events_v2()
        .head("agt_fixture")
        .await
        .unwrap();
    assert_eq!(value.through_cursor.as_str(), cursor(9));
    assert_eq!(
        value
            .active_turn
            .unwrap()
            .response_binding
            .unwrap()
            .command_id,
        "cmd_fixture"
    );
    let mut idle = head();
    idle["active_turn"] = Value::Null;
    server.expect(expected(&idle));
    assert!(
        server
            .client
            .agent_events_v2()
            .head("agt_fixture")
            .await
            .unwrap()
            .active_turn
            .is_none()
    );
    server.assert_drained();
}

#[tokio::test]
async fn refuses_mismatched_identity_turn_boundary_and_binding() {
    let mut cases = Vec::new();
    for (field, value) in [
        ("object", json!("cma.agent.event_page")),
        ("schema_version", json!(2)),
        ("agent_id", json!("agt_other")),
        ("incarnation", json!("b".repeat(64))),
        ("through_cursor", json!(cursor(6))),
        ("active_turn", json!({})),
        ("unexpected", json!(true)),
    ] {
        let mut changed = head();
        changed[field] = value;
        cases.push(changed);
    }
    for (field, value) in [
        ("cursor", json!(cursor(8))),
        ("sequence", json!(0)),
        ("appended_at", json!("")),
        ("kind", Value::Null),
        ("kind", json!({"type":"turn_completed","turn_sequence":7})),
        ("kind", json!({"type":"turn_started","turn_sequence":6})),
        (
            "response_binding",
            json!({"response_id":"resp_fixture","command_id":"cmd_fixture","turn_sequence":6}),
        ),
    ] {
        let mut changed = head();
        changed["active_turn"][field] = value;
        cases.push(changed);
    }
    let mut missing = head();
    missing.as_object_mut().unwrap().remove("active_turn");
    cases.push(missing);
    for changed in cases {
        let server = WireServer::new().await;
        server.expect(expected(&changed));
        assert!(matches!(
            server.client.agent_events_v2().head("agt_fixture").await,
            Err(Error::Protocol { .. })
        ));
        server.assert_drained();
    }
}

#[tokio::test]
async fn validates_agent_before_network_io() {
    let server = WireServer::new().await;
    assert!(matches!(
        server.client.agent_events_v2().head("").await,
        Err(Error::Validation { .. })
    ));
    assert_eq!(server.request_count(), 0);
    server.assert_drained();
}
