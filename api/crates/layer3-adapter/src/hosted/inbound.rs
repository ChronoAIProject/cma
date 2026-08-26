//! What a bridge says, read back into the contract's vocabulary.
//!
//! Split from the client so both files stay within the project's size limit,
//! on the division the protocol already has: that file asks and this one
//! listens. Everything here is driven by a frame that arrived — an answer to
//! something outstanding, an update to pass up, a permission request to hold
//! until a person answers it, or a method this client does not serve.

use tokio::sync::mpsc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
                    return;
                }
                if self
                    .orphaned_prompt_response
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    // The old Bridge owned the prompt waiter and disappeared
                    // with it. A restored permission answer is what allows this
                    // response to exist, so carry its outcome through the new
                    // update stream before releasing the turn-scoped cache.
                    self.answered.lock().await.clear();
                    let outcome = super::turn::turn_outcome(frame);
                    let _ = updates.send(outcome.into_update()).await;
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
            Some("elicitation/create") => self.sign_in(&frame, updates).await,
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

    /// Pass a sign-in the kernel is waiting on up to whoever can complete it.
    ///
    /// Reached only on an own-account session: the kernel wants its owner to
    /// open a page and enter a code, and it will block until they have. The
    /// address and the code go up so an interface can show them.
    ///
    /// Accepted immediately rather than after the person has finished. The
    /// answer means "this client is able to put that in front of somebody",
    /// not "they are done" — the kernel discovers that for itself by polling,
    /// and holding the reply until then would leave it waiting on us while we
    /// waited on it. A refusal here is what cancels the sign-in, so it is sent
    /// only when there is genuinely nothing that could show it.
    ///
    /// Nothing that comes back this way is a credential. What the sign-in
    /// produces is written inside the sandbox by the kernel, which is the
    /// whole basis of `CredentialMode::Own`'s promise.
    async fn sign_in(&self, frame: &serde_json::Value, updates: &mpsc::Sender<ConversationUpdate>) {
        let asked = sign_in_from(frame.get("params"));
        let Some((url, code, message)) = asked else {
            // Nothing to show. Refused rather than accepted, so the kernel
            // stops waiting on a sign-in that was never going to appear.
            tracing::warn!("a sign-in request carrying no address");
            if let Some(id) = frame.get("id") {
                let _ = self
                    .send(&serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "action": "decline" },
                    }))
                    .await;
            }
            return;
        };
        tracing::info!(%url, "the agent is waiting for its owner to sign in");
        let _ = updates
            .send(ConversationUpdate::SignInRequired { url, code, message })
            .await;

        if let Some(id) = frame.get("id") {
            let _ = self
                .send(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "action": "accept" },
                }))
                .await;
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
        let locator = permission_locator(request.session_id.0.as_ref(), &call, &request.options);
        // Stable across delivery of the same blocked call on a replacement
        // socket. The locator carries only the conversation scope, call id and
        // normalized offered choices; hashing it avoids exposing them in the
        // public id.
        let id = permission_id(&locator);
        let repeated_answer = self.answered.lock().await.get(&id).cloned();
        if let Some(answer) = repeated_answer {
            let _ = self.send(&answer).await;
            return;
        }
        // The prompt counter is raised before a local prompt is written, so a
        // permission request caused by that prompt is distinguishable from one
        // replayed onto a replacement socket. Store the distinction on the
        // request: a different overlapping prompt may finish before its owner
        // eventually answers this one.
        let owns_prompt_waiter = self
            .owned_prompt_waiters
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0;
        self.blocked.lock().await.insert(
            id.clone(),
            Blocked {
                call,
                options: request.options.clone(),
                owns_prompt_waiter,
            },
        );
        let _ = updates
            .send(ConversationUpdate::Asked(PermissionRequest {
                id,
                locator: Some(locator),
                request,
            }))
            .await;
    }
}

const PERMISSION_LOCATOR_VERSION: u8 = 1;
const PERMISSION_ID_DOMAIN: &[u8] = b"chrono-hosted-permission-v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredPermissionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

impl StoredPermissionKind {
    fn from_option(kind: PermissionOptionKind) -> Option<Self> {
        match kind {
            PermissionOptionKind::AllowOnce => Some(Self::AllowOnce),
            PermissionOptionKind::AllowAlways => Some(Self::AllowAlways),
            PermissionOptionKind::RejectOnce => Some(Self::RejectOnce),
            PermissionOptionKind::RejectAlways => Some(Self::RejectAlways),
            // A future intent this build cannot express is not a route it can
            // select. Omitting it produces the same safe cancellation as the
            // live matcher when no known option fits.
            _ => None,
        }
    }
}

impl From<StoredPermissionKind> for PermissionOptionKind {
    fn from(kind: StoredPermissionKind) -> Self {
        match kind {
            StoredPermissionKind::AllowOnce => Self::AllowOnce,
            StoredPermissionKind::AllowAlways => Self::AllowAlways,
            StoredPermissionKind::RejectOnce => Self::RejectOnce,
            StoredPermissionKind::RejectAlways => Self::RejectAlways,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct StoredPermissionOption {
    id: String,
    kind: StoredPermissionKind,
}

#[derive(Serialize, Deserialize)]
struct HostedPermissionLocator {
    version: u8,
    session: String,
    call: serde_json::Value,
    options: Vec<StoredPermissionOption>,
}

fn permission_locator(
    session: &str,
    call: &serde_json::Value,
    options: &[PermissionOption],
) -> crate::PermissionLocator {
    let locator = HostedPermissionLocator {
        version: PERMISSION_LOCATOR_VERSION,
        session: session.to_string(),
        call: call.clone(),
        options: options
            .iter()
            .filter_map(|option| {
                Some(StoredPermissionOption {
                    id: option.option_id.0.to_string(),
                    kind: StoredPermissionKind::from_option(option.kind)?,
                })
            })
            .collect(),
    };
    crate::PermissionLocator::new(
        serde_json::to_string(&locator).expect("a permission locator is always serializable"),
    )
}

pub(super) fn permission_id(locator: &crate::PermissionLocator) -> String {
    let mut digest = Sha256::new();
    digest.update(PERMISSION_ID_DOMAIN);
    digest.update(locator.as_str().as_bytes());
    digest
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut rendered, byte| {
            use std::fmt::Write as _;
            let _ = write!(&mut rendered, "{byte:02x}");
            rendered
        })
}

pub(super) fn blocked_from_locator(
    id: &str,
    locator: &crate::PermissionLocator,
    session: &str,
) -> Result<super::acp::Blocked, crate::AdapterError> {
    if permission_id(locator) != id {
        return Err(crate::AdapterError::Rejected(
            "the permission locator does not belong to this request".to_string(),
        ));
    }
    let stored: HostedPermissionLocator = serde_json::from_str(locator.as_str()).map_err(|_| {
        crate::AdapterError::Rejected("the permission locator is not readable".to_string())
    })?;
    if stored.version != PERMISSION_LOCATOR_VERSION {
        return Err(crate::AdapterError::Rejected(format!(
            "permission locator version {} is not supported",
            stored.version
        )));
    }
    if stored.session != session {
        return Err(crate::AdapterError::Rejected(
            "the permission locator belongs to a different conversation".to_string(),
        ));
    }
    Ok(super::acp::Blocked {
        call: stored.call,
        options: stored
            .options
            .into_iter()
            .map(|option| {
                PermissionOption::new(
                    crate::PermissionOptionId::new(option.id),
                    String::new(),
                    option.kind.into(),
                )
            })
            .collect(),
        owns_prompt_waiter: false,
    })
}

/// Read a sign-in request into the three things a person needs.
///
/// Separated from the sending so it can be tested without a bridge: what a
/// client shows somebody is decided here, and getting it wrong shows them a
/// card they cannot act on.
///
/// The code is read only from where it is declared. Some bridges carry it in
/// its own field and some only inside the sentence; parsing it out of prose
/// would break the first time the wording changed, and the sentence is shown
/// whole either way.
pub(crate) fn sign_in_from(
    params: Option<&serde_json::Value>,
) -> Option<(String, Option<String>, Option<String>)> {
    let text = |key: &str| {
        params
            .and_then(|p| p.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    // No address, nothing to show. Everything else is optional.
    let url = text("url")?;
    let code = text("userCode").or_else(|| text("code"));
    Some((url, code, text("message")))
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
