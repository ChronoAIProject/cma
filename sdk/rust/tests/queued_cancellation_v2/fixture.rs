use super::*;
pub const AGENT: &str = "agt_fixture";
pub const RESPONSE: &str = "resp_control_source";
pub const SOURCE: &str = "cmd_control_source";
pub const ROUTE: &str =
    "/api/v2/agents/agt_fixture/responses/resp_control_source/queued-cancellation";
pub fn source() -> Value {
    let corpus: Value = serde_json::from_str(include_str!("../../../../api/crates/layer1-public/tests/fixtures/public-contract/response-controls-v2.json")).unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "response-controls-v2-queued-observe")
        .unwrap()["response"]["body"]["response"]
        .clone()
}
pub fn intent() -> QueuedCancellationRequest {
    QueuedCancellationRequest {
        source_command_id: SOURCE.into(),
        expected_command_revision: 1,
    }
}
pub fn observation() -> Value {
    json!({"object":"cma.response.queued_cancellation.observation", "response":source(), "command_revision":1, "cancellable":true})
}
pub fn receipt() -> Value {
    let mut cancellation = source();
    cancellation["receipt"]["command_id"] = json!("cmd_queued_cancellation");
    cancellation["receipt"]["response_id"] = Value::Null;
    cancellation["receipt"]["agent_order"] = json!(2);
    cancellation["status"] = json!("completed");
    let mut response = source();
    response["status"] = json!("cancelled");
    json!({"object":"cma.response.queued_cancellation", "cancellation":cancellation, "response":response,
        "target":{"response_id":RESPONSE,"source_command_id":SOURCE,"expected_command_revision":1}})
}
pub fn expected(value: Value, status: u16, key: &str) -> wire::Expected {
    let mut expected = wire::json(
        "POST",
        ROUTE,
        Some(serde_json::to_value(intent()).unwrap()),
        status,
        value,
        true,
    );
    expected
        .headers
        .insert("idempotency-key".into(), key.into());
    expected
}
