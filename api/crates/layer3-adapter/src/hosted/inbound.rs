//! What a bridge says, read back into the contract's vocabulary.
//!
//! Split from the client so both files stay within the project's size limit,
//! on the division the protocol already has: that file asks and this one
//! listens. Everything here is driven by a frame that arrived — an answer to
//! something outstanding, an update to pass up, a permission request to hold
//! until a person answers it, or a method this client does not serve.

use tokio::sync::mpsc;

use super::acp::{Blocked, Bridge};
use crate::{
    ConversationUpdate, PermissionDecision, PermissionOption, PermissionOptionKind,
    PermissionRequest, SessionUpdate,
};

impl Bridge {
    /// Act on one frame the bridge sent.
    pub(super) async fn receive(&self, payload: &[u8], updates: &mpsc::Sender<ConversationUpdate>) {
        let Ok(frame) = serde_json::from_slice::<serde_json::Value>(payload) else {
            // Unreadable rather than unfamiliar. Logged without its content:
            // a conversation frame carries what somebody said to their agent.
            tracing::warn!(bytes = payload.len(), "a conversation frame was not json");
            return;
        };

        // An answer to something we asked.
        if let Some(id) = frame.get("id").and_then(serde_json::Value::as_i64) {
            if frame.get("method").is_none() {
                let waiting = self.pending.lock().await.remove(&id);
                if let Some(waiting) = waiting {
                    let _ = waiting.send(frame);
                }
                return;
            }
        }

        match frame.get("method").and_then(serde_json::Value::as_str) {
            Some("session/update") => {
                let Some(update) = frame.pointer("/params/update") else {
                    return;
                };
                match serde_json::from_value::<SessionUpdate>(update.clone()) {
                    Ok(update) => {
                        let _ = updates.send(ConversationUpdate::Said(update)).await;
                    }
                    // A shape of update this build has no type for. Skipped
                    // rather than failing the conversation: a newer bridge
                    // saying something new must not end a live session.
                    Err(e) => tracing::debug!(error = %e, "an update this build cannot read"),
                }
            }
            Some("session/request_permission") => self.blocked_on(frame, updates).await,
            // Everything else the bridge may say to a client — filesystem
            // requests, terminal requests — is not offered by this client's
            // declared capabilities and is answered as unsupported rather than
            // ignored, so the bridge is not left waiting.
            Some(method) => {
                if let Some(id) = frame.get("id") {
                    tracing::info!(%method, "refusing a request this client does not serve");
                    let refusal = serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": "not offered by this client" },
                    });
                    let _ = self.send(&refusal).await;
                }
            }
            None => {}
        }
    }

    /// Record a permission request and pass it up for somebody to answer.
    async fn blocked_on(
        &self,
        frame: serde_json::Value,
        updates: &mpsc::Sender<ConversationUpdate>,
    ) {
        let Some(params) = frame.get("params") else {
            return;
        };
        let Ok(request) = serde_json::from_value::<crate::RequestPermissionRequest>(params.clone())
        else {
            tracing::warn!("a permission request this build cannot read");
            return;
        };
        let Some(call) = frame.get("id").cloned() else {
            return;
        };
        // Ours rather than the protocol's, because the protocol keeps this at
        // the JSON-RPC layer and the core needs something durable to record
        // against an approval event and accept back minutes later.
        let id = uuid::Uuid::new_v4().simple().to_string();
        self.blocked.lock().await.insert(
            id.clone(),
            Blocked {
                call,
                options: request.options.clone(),
            },
        );
        let _ = updates
            .send(ConversationUpdate::Asked(PermissionRequest { id, request }))
            .await;
    }
}

/// Render an owner's intent into one of the options a bridge offered.
///
/// The options are the bridge's own and differ per request, so the intent is
/// matched against what was actually on offer rather than against a fixed set.
/// An intent with nothing to match is cancelled, which every ACP agent
/// understands and which stops the turn rather than letting it proceed — the
/// safe direction when the answer was "no" and we cannot say which kind.
pub(crate) fn outcome_for(
    decision: PermissionDecision,
    options: &[PermissionOption],
) -> serde_json::Value {
    let wanted: &[PermissionOptionKind] = match decision {
        PermissionDecision::Allow => &[
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ],
        PermissionDecision::Deny => &[
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ],
        PermissionDecision::Abort => &[],
    };
    for kind in wanted {
        if let Some(option) = options.iter().find(|o| &o.kind == kind) {
            return serde_json::json!({
                "outcome": "selected",
                "optionId": option.option_id.0.to_string(),
            });
        }
    }
    serde_json::json!({ "outcome": "cancelled" })
}

#[cfg(test)]
#[path = "inbound_tests.rs"]
mod inbound_tests;
