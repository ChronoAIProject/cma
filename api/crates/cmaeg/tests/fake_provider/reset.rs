//! Retained reset receipts are scoped exactly as CMA scopes owner operations.
use super::*;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct Controls {
    pub enabled: Arc<AtomicBool>,
    pub lose_once: Arc<AtomicBool>,
    pub response: Arc<Mutex<Option<(u16, Value)>>>,
    pub gate: Option<Arc<Semaphore>>,
}
#[derive(Clone)]
pub(super) struct Receipt {
    binding: String,
    conversation: String,
    body: Value,
    result: Value,
}
pub(super) type Ledger = BTreeMap<(String, String), Receipt>;

#[allow(dead_code)] // Shared fake is compiled by suites that do not perform repairs.
impl FakeProvider {
    pub fn reset_controls(&self) -> Controls {
        self.state.config.reset.clone()
    }
    pub fn reset_count(&self) -> usize {
        self.state.resets.lock().unwrap().len()
    }
}

pub(super) async fn reset(
    State(state): State<Arc<StateData>>,
    Path((binding, conversation)): Path<(String, String)>,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let authorized = headers
        .get("authorization")
        .is_some_and(|v| v == "Bearer cma-service");
    record(
        &state,
        "POST",
        format!("/api/v1/cmaeg/bindings/{binding}/conversations/{conversation}/resets"),
        headers,
        bytes.clone(),
    );
    if !authorized {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    if let Some((status, body)) = state.config.reset.response.lock().unwrap().clone() {
        return (StatusCode::from_u16(status).unwrap(), Json(body)).into_response();
    }
    let result = {
        let mut ledger = state.resets.lock().unwrap();
        let scope = (
            body["idempotency_binding_id"].as_str().unwrap().to_owned(),
            key,
        );
        if let Some(receipt) = ledger.get(&scope) {
            if receipt.body != body
                || receipt.binding != binding
                || receipt.conversation != conversation
            {
                return refusal("partition_reset_key_conflict");
            }
            receipt.result.clone()
        } else {
            if !state.config.reset.enabled.load(Ordering::SeqCst) {
                return refusal("partition_reset_disabled");
            }
            let incarnation = ledger
                .values()
                .filter(|r| r.binding == binding && r.conversation == conversation)
                .count()
                + 1;
            let result = json!({"binding_id":binding,"generation":1,"conversation_id":conversation,
                "incarnation":incarnation,"previous_agent_id":body["expected_agent_id"],
                "previous_conversation_id":if incarnation == 1 { conversation.clone() } else { format!("internal_{}", incarnation-1) },
                "successor_agent_id":format!("agt_{}",uuid::Uuid::new_v4().simple()),"state":"reserved"});
            ledger.insert(
                scope,
                Receipt {
                    binding,
                    conversation,
                    body,
                    result: result.clone(),
                },
            );
            result
        }
    };
    if let Some(gate) = &state.config.reset.gate {
        gate.acquire().await.unwrap().forget();
    }
    if state.config.reset.lose_once.swap(false, Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    Json(result).into_response()
}
fn refusal(code: &str) -> Response {
    (StatusCode::CONFLICT, Json(json!({"code":code}))).into_response()
}
pub(super) fn inspection(state: &StateData, binding: &str, conversation: &str) -> Option<Value> {
    state.resets.lock().unwrap().values()
        .filter(|r| r.binding == binding && r.conversation == conversation)
        .max_by_key(|r| r.result["incarnation"].as_u64().unwrap())
        .map(|r| json!({"binding_id":binding,"generation":1,"conversation_id":conversation,
            "incarnation":r.result["incarnation"],"agent_id":r.result["successor_agent_id"],"state":"reserved","agent_status":null}))
}
