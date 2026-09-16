use super::wire::{self, WireServer};
use cma_sdk::{IdempotencyKey, bridges::*};
use serde_json::{Value, json};
pub const BRIDGE: &str = "bridge-cleanup";
pub fn allocation() -> String {
    format!("bral_{}", "1".repeat(64))
}
pub fn key() -> IdempotencyKey {
    IdempotencyKey::new("retained-cleanup").unwrap()
}
pub fn request() -> BridgeChildCleanupRequest {
    BridgeChildCleanupRequest {
        expected_bridge_revision: 3,
        expected_allocation_revision: 4,
        action: BridgeChildCleanupAction::RemoveChild,
    }
}
pub fn target() -> Value {
    json!({"sandbox_id":"sbx_parent","workspace_id":"wks_parent","worktree_id":"wkt_child","agent_id":"agt_child"})
}
pub fn status() -> Value {
    json!({"bridge_id":BRIDGE,"allocation_id":allocation(),"generation":1,"allocation_revision":4,"target":target(),
"receipt":{"id":format!("bclj_{}","2".repeat(64)),"bridge_id":BRIDGE,"allocation_id":allocation(),"generation":1,"target":target(),"request":request(),"requested_by":{"kind":"api","subject":"manager","delegation_id":null},"accepted_at_ms":200},
"progress":"requested","holds":["uncertain_reply"],"observed_at_ms":100,"agent_capacity_released":false,"workspace_retained":true,"history_retained":true})
}
pub fn row() -> Value {
    json!({"id":allocation(),"bridge_id":BRIDGE,"generation":1,"revision":4,"target":target(),"placement":"reuse_parent","state":"retired"})
}
pub fn path() -> String {
    format!(
        "/api/v1/bridges/{BRIDGE}/allocations/{}/child-cleanup",
        allocation()
    )
}
pub fn expect(server: &WireServer, method: &str, value: Value, status: u16) {
    let mut wire = wire::json(
        method,
        &path(),
        (method == "POST").then(|| serde_json::to_value(request()).unwrap()),
        status,
        value,
        true,
    );
    if method == "POST" {
        wire.headers
            .insert("idempotency-key".into(), key().as_str().into());
    }
    server.expect(wire);
}
