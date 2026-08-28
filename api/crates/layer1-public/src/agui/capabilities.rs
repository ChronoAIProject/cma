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
    Json(document())
}

/// The capability document before the HTTP wrapper is applied.
///
/// Kept as one value constructor so the served route, documentation mirror,
/// and cross-language fixture are assertions about the same source.
pub(super) fn document() -> serde_json::Value {
    serde_json::json!({
        "protocol": "ag-ui",
        "transports": ["sse"],
        // The machine-readable form of the document's §3, and the contract.
        // A client reads this rather than assuming, because the set grows.
        "events": {
            "RUN_STARTED": true, "RUN_FINISHED": true, "RUN_ERROR": true,
            "TEXT_MESSAGE_START": true, "TEXT_MESSAGE_CONTENT": true, "TEXT_MESSAGE_END": true,
            "TOOL_CALL_START": true, "TOOL_CALL_ARGS": true, "TOOL_CALL_END": true,
            "TOOL_CALL_RESULT": true,
            // The agent's reasoning, on the live lane. Only the content event:
            // this service sees a stream of thoughts and not where one begins
            // or ends, so `REASONING_START` and `REASONING_END` would be a
            // guess. A client brackets on the run's own lifecycle events,
            // which are exact.
            "REASONING_MESSAGE_CONTENT": true,
            "REASONING_START": false, "REASONING_END": false,
            "REASONING_MESSAGE_START": false, "REASONING_MESSAGE_END": false,
            "REASONING_MESSAGE_CHUNK": false, "REASONING_ENCRYPTED_VALUE": false,
            // The deprecated spelling of the family above. Declared false
            // rather than omitted, so a client written against the older names
            // is told they are not coming rather than left waiting on them.
            "THINKING_START": false, "THINKING_END": false,
            "THINKING_TEXT_MESSAGE_START": false, "THINKING_TEXT_MESSAGE_CONTENT": false,
            "THINKING_TEXT_MESSAGE_END": false,
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
            // because none of these is part of its vocabulary. The terminal is
            // false here and true elsewhere: this interface cannot carry one.
            //
            // The plane change is declared false rather than left out, and the
            // two readings differ: absent says this build has not heard of it,
            // and false says it exists and this interface does not offer it.
            // A client is told where it does live — a state delta says when a
            // session's plane changed, and REST is where one is asked for.
            "custom": {
                "cma:terminal": false,
                "cma:usage": true,
                "cma:changePlane": false,
                "cma:planeChangeObserved": true,
                "cma:resources": true,
                // The run stream also carries what is happening right now, not
                // only what has happened. Those frames are marked
                // `cma:live` and carry no SSE id: they are not in the
                // durable log and a client must never resume from one.
                // Declared so a client can size its handling rather than
                // discover the marker on a frame it has already mishandled.
                "cma:liveFrames": true
            }
        },
        "conformsTo": [
            "https://spec.chrono-ai.fun/cma/1.0/conf/agui-core",
            "https://spec.chrono-ai.fun/cma/1.0/conf/approvals"
        ]
    })
}

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod capabilities_tests;
