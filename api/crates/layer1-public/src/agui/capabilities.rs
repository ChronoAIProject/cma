//! What this interface can do, in the form a client reads rather than guesses.
//!
//! Two claims, and they are different. The `events` map says what this
//! interface may emit for *some* session — the vocabulary a client must be
//! ready for. What a *particular* session can be asked for is the session's
//! own granted set, reported on the session object, and a request outside it
//! is refused by name.
//!
//! `docs/API_AGUI.md` §6 is the contract. The document and this handler must
//! agree, and a test asserts the shape rather than trusting that they do.

use axum::Json;

/// The capability document.
pub(super) async fn declare() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "protocol": "ag-ui",
        "transports": ["sse"],
        // The machine-readable form of the document's §3, and the contract.
        // A client reads this rather than assuming, because the set grows.
        "events": {
            "RUN_STARTED": true, "RUN_FINISHED": true, "RUN_ERROR": true,
            "TEXT_MESSAGE_START": true, "TEXT_MESSAGE_CONTENT": true, "TEXT_MESSAGE_END": true,
            "TOOL_CALL_START": true, "TOOL_CALL_ARGS": false, "TOOL_CALL_END": true,
            "TOOL_CALL_RESULT": false, "THINKING_START": false, "THINKING_END": false,
            "STATE_SNAPSHOT": false, "STATE_DELTA": true
        },
        "features": {
            // Resumable through the durable log rather than by retrying the
            // run, which would start a second turn. §7 of the document.
            "transport": {
                "streaming": true, "resumable": true,
                "websocket": false, "pushNotifications": false
            },
            "state": { "snapshots": false, "deltas": true, "persistentState": true },
            "humanInTheLoop": {
                "supported": true, "approvals": true, "interrupts": true,
                "approveWithEdits": false, "interventions": false
            },
            "execution": { "codeExecution": true, "sandboxed": true },
            // Named with the prefix the protocol reserves for a vendor's own,
            // because neither is part of its vocabulary. The terminal is false
            // here and true elsewhere: this interface cannot carry one.
            "custom": { "chrono:terminal": false, "chrono:usage": true }
        },
        "conformsTo": [
            "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/agui-core",
            "https://spec.chrono-ai.fun/chrono-bot/1.0/conf/approvals"
        ]
    }))
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod capabilities_tests;
