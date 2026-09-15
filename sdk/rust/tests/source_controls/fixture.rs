use super::wire::{self, WireServer};
use cma_sdk::{IdempotencyKey, bridges::*};
use serde_json::Value;
pub fn receipt() -> Value {
    let corpus: Value = serde_json::from_str(include_str!("../../../../api/crates/layer1-public/tests/fixtures/public-contract/bridge-source-consent.json")).unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == "source-consent-enable")
        .unwrap()["response"]["body"]
        .clone()
}
pub fn grant() -> String {
    receipt()["result"]["grant_id"].as_str().unwrap().to_owned()
}
pub fn request() -> BridgeSourceControlRequest {
    serde_json::from_value(receipt()["request"].clone()).unwrap()
}
pub fn key() -> IdempotencyKey {
    IdempotencyKey::new("original-consent").unwrap()
}
pub fn path() -> String {
    format!("/api/v1/bridge-invoke-grants/{}/source-controls", grant())
}
pub fn expect(server: &WireServer, method: &str, body: Option<Value>, value: Value, status: u16) {
    let mut expectation = wire::json(method, &path(), body, status, value, true);
    if method == "PUT" {
        expectation
            .headers
            .insert("idempotency-key".into(), key().as_str().into());
    }
    server.expect(expectation);
}
