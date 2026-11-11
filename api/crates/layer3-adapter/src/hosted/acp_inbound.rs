use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::acp::{Blocked, Bridge};
use super::update_relay::UpdateRelay;
use crate::{
    ConversationUpdate, PermissionDecision, PermissionOption, PermissionOptionKind, SessionUpdate,
};

const PERMISSION_LOCATOR_VERSION: u8 = 1;
const PERMISSION_ID_DOMAIN: &[u8] = b"chrono-hosted-permission-v1\0";
const MAX_PERMISSION_CALL_BYTES: usize = 256;

impl Bridge {
    pub(super) async fn receive(&self, payload: &[u8], updates: &UpdateRelay) -> crate::Result<()> {
        let Ok(frame) = serde_json::from_slice::<serde_json::Value>(payload) else {
            tracing::warn!(bytes = payload.len(), "ACP conversation frame was not JSON");
            return Ok(());
        };

        if let Some(id) = frame.get("id") {
            if frame.get("method").is_none() {
                let waiting = match id.as_str() {
                    Some(id) => self.pending.lock().await.remove(id),
                    None => None,
                };
                if let Some(waiting) = waiting {
                    if waiting.prompt {
                        let prompt_id = id.as_str().and_then(crate::PromptIdentity::parse);
                        if let Some(id) = id.as_str() {
                            self.mark_prompt_completed(id).await;
                        }
                        updates.send(super::acp_turn::scoped_turn_outcome(
                            super::acp_turn::turn_outcome(frame.clone()),
                            true,
                            prompt_id,
                        ));
                    }
                    let _ = waiting.answer.send(frame);
                    return Ok(());
                }
                if let Some(id) = id.as_str() {
                    if self.recover_prompt_result(id, frame.clone(), updates).await {
                        self.orphaned_prompt_response
                            .store(false, std::sync::atomic::Ordering::SeqCst);
                        self.answered.lock().await.clear();
                        return Ok(());
                    }
                    let already_completed = self.prompt_was_completed(id).await;
                    if !already_completed {
                        // The response ID is the native authority. A passive
                        // attachment may have no local waiter, but it must
                        // still carry a valid terminal identity to the durable
                        // recorder before it can ACK the retained frame.
                        if let Some(prompt_id) = crate::PromptIdentity::parse(id) {
                            if let Some(outcome) = prompt_outcome(&frame) {
                                self.mark_prompt_completed(id).await;
                                self.orphaned_prompt_response
                                    .store(false, std::sync::atomic::Ordering::SeqCst);
                                updates.send(super::acp_turn::scoped_turn_outcome(
                                    outcome,
                                    true,
                                    Some(prompt_id),
                                ));
                                return Ok(());
                            }
                        }
                    }
                }
                if id.as_i64().is_some()
                    && self
                        .orphaned_prompt_response
                        .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    // A restored permission has no native terminal identity
                    // on this legacy numeric path. Consume the compatibility
                    // marker, but never let an unrelated numeric response
                    // close whichever turn happens to be active.
                    self.answered.lock().await.clear();
                }
                return Ok(());
            }
        }

        match frame.get("method").and_then(serde_json::Value::as_str) {
            Some("cma/codex/credentials") => self.subscription_credential(&frame).await,
            Some("session/update") => self.session_update(&frame, updates).await,
            Some("session/request_permission") => self.blocked_on(frame, updates).await?,
            Some("elicitation/create") => self.sign_in(&frame, updates).await,
            Some(method) => {
                if let Some(id) = frame.get("id") {
                    tracing::info!(%method, "refusing ACP request not offered by this client");
                    let _ = self
                        .send(&serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32601,
                                "message": "not offered by this client"
                            },
                        }))
                        .await;
                }
            }
            None => {}
        }
        Ok(())
    }

    async fn subscription_credential(&self, frame: &serde_json::Value) {
        let Some(id) = frame.get("id").filter(|id| valid_call(id)) else {
            return;
        };
        let answer = match (&self.subscription, frame.get("params")) {
            (Some(binding), Some(params)) if params.is_object() => binding.credential(params).await,
            _ => Err(crate::AdapterError::Rejected(
                "subscription authority is not bound".into(),
            )),
        };
        let frame = match answer {
            Ok(result) => serde_json::json!({"jsonrpc":"2.0", "id":id, "result":result}),
            Err(_) => serde_json::json!({"jsonrpc":"2.0", "id":id, "error":{
                "code":-32001, "message":"Re-login with Codex subscription to continue"
            }}),
        };
        let _ = self.send(&frame).await;
    }

    async fn session_update(&self, frame: &serde_json::Value, updates: &UpdateRelay) {
        let Some(session) = frame
            .pointer("/params/sessionId")
            .and_then(serde_json::Value::as_str)
        else {
            tracing::warn!("ACP session update named no conversation");
            return;
        };
        let Some(value) = frame.pointer("/params/update") else {
            return;
        };
        self.observe_context_goal(session, value).await;
        if let Some(update) = super::acp_observations::observation(value) {
            self.route_update(session, update, updates).await;
            return;
        }
        match serde_json::from_value::<SessionUpdate>(value.clone()) {
            Ok(update) => {
                self.route_update(session, ConversationUpdate::Said(update), updates)
                    .await;
            }
            Err(error) => {
                tracing::debug!(%error, "ACP update is newer than this client understands");
            }
        }
    }

    async fn sign_in(&self, frame: &serde_json::Value, updates: &UpdateRelay) {
        let Some((url, code, message)) = sign_in_from(frame.get("params")) else {
            tracing::warn!("ACP sign-in request contained no URL");
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
        updates.send(ConversationUpdate::SignInRequired { url, code, message });
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

    pub(super) async fn refuse_invalid_call(&self, frame: &serde_json::Value) {
        if let Some(id) = frame.get("id") {
            let _ = self
                .send(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32602, "message": "conversation does not match" },
                }))
                .await;
        }
    }
}

pub(super) fn scope_activity(update: ConversationUpdate, owns_prompt: bool) -> ConversationUpdate {
    let is_activity = matches!(
        update.turn_evidence(),
        Some(crate::TurnEvidence::Activity(_))
    );
    if is_activity && !owns_prompt {
        update.without_prompt_evidence()
    } else {
        update
    }
}

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

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPermissionOption {
    id: String,
    kind: StoredPermissionKind,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HostedPermissionLocator {
    version: u8,
    session: String,
    call: serde_json::Value,
    options: Vec<StoredPermissionOption>,
}

pub(super) fn permission_locator(
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
        serde_json::to_string(&locator).expect("permission locator is serializable"),
    )
}

pub(super) fn permission_id(locator: &crate::PermissionLocator) -> String {
    let mut digest = Sha256::new();
    digest.update(PERMISSION_ID_DOMAIN);
    digest.update(locator.as_str().as_bytes());
    hex(&digest.finalize())
}

pub(super) fn blocked_from_locator(
    id: &str,
    locator: &crate::PermissionLocator,
    session: &str,
) -> crate::Result<Blocked> {
    if permission_id(locator) != id {
        return Err(crate::AdapterError::Rejected(
            "permission locator does not belong to this request".into(),
        ));
    }
    let stored: HostedPermissionLocator = serde_json::from_str(locator.as_str())
        .map_err(|_| crate::AdapterError::Rejected("permission locator is not readable".into()))?;
    if stored.version != PERMISSION_LOCATOR_VERSION
        || stored.session != session
        || !valid_call(&stored.call)
    {
        return Err(crate::AdapterError::Rejected(
            "permission locator belongs to another conversation or version".into(),
        ));
    }
    Ok(Blocked {
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

pub(crate) fn sign_in_from(
    params: Option<&serde_json::Value>,
) -> Option<(String, Option<String>, Option<String>)> {
    let text = |key: &str| {
        params
            .and_then(|params| params.get(key))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let url = text("url")?;
    Some((
        url,
        text("userCode").or_else(|| text("code")),
        text("message"),
    ))
}

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
        if let Some(option) = options.iter().find(|option| &option.kind == kind) {
            return serde_json::json!({
                "outcome": "selected",
                "optionId": option.option_id.0.to_string(),
            });
        }
    }
    serde_json::json!({ "outcome": "cancelled" })
}

fn prompt_outcome(frame: &serde_json::Value) -> Option<super::acp::Outcome> {
    let terminal = frame.get("error").is_some()
        || frame
            .get("result")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|result| result.contains_key("stopReason"));
    terminal.then(|| super::acp_turn::turn_outcome(frame.clone()))
}

pub(super) fn valid_call(call: &serde_json::Value) -> bool {
    matches!(
        call,
        serde_json::Value::String(_) | serde_json::Value::Number(_)
    ) && call.to_string().len() <= MAX_PERMISSION_CALL_BYTES
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}
