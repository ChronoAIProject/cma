use axum::{extract::State, Json};
use serde_json::{json, Value};

#[utoipa::path(get, path = "/api/v1/agui/capabilities", tag = "ag-ui",
    responses((status = 200, body = Value)))]
pub(super) async fn declare(State(state): State<crate::AgentConversationState>) -> Json<Value> {
    let mut value = document();
    value["features"]["custom"]["cma:liveFrames"] = state.has_live_updates().into();
    value["events"]["REASONING_MESSAGE_CONTENT"] = state.has_live_updates().into();
    Json(value)
}

pub(super) fn document() -> Value {
    json!({
        "protocol": "ag-ui", "transports": ["sse"],
        "events": {
            "RUN_STARTED": true, "RUN_FINISHED": true, "RUN_ERROR": true,
            "TEXT_MESSAGE_START": true, "TEXT_MESSAGE_CONTENT": true, "TEXT_MESSAGE_END": true,
            "TOOL_CALL_START": true, "TOOL_CALL_ARGS": false, "TOOL_CALL_END": true,
            "TOOL_CALL_RESULT": false, "REASONING_MESSAGE_CONTENT": false,
            "REASONING_START": false, "REASONING_END": false,
            "REASONING_MESSAGE_START": false, "REASONING_MESSAGE_END": false,
            "REASONING_MESSAGE_CHUNK": false, "REASONING_ENCRYPTED_VALUE": false,
            "THINKING_START": false, "THINKING_END": false,
            "THINKING_TEXT_MESSAGE_START": false, "THINKING_TEXT_MESSAGE_CONTENT": false,
            "THINKING_TEXT_MESSAGE_END": false,
            "STATE_SNAPSHOT": false, "STATE_DELTA": true
        },
        "features": {
            "transport": {"streaming": true, "resumable": true, "websocket": false, "pushNotifications": false},
            "state": {"snapshots": false, "deltas": true, "persistentState": true},
            "humanInTheLoop": {"supported": true, "approvals": true, "interrupts": true,
                "approveWithEdits": false, "interventions": false},
            "execution": {"codeExecution": true, "sandboxed": true},
            "custom": {
                "cma:terminal": false, "cma:usage": true, "cma:liveFrames": false,
                "cma:sandbox": {"snapshot": true, "status": true, "workspace": true,
                    "worktree": true, "pausedNotice": true, "firstMessageDelivery": true, "mutations": false}
            }
        },
        "conformsTo": ["https://spec.chrono-ai.fun/cma/1.0/conf/agui-core",
            "https://spec.chrono-ai.fun/cma/1.0/conf/approvals"]
    })
}
