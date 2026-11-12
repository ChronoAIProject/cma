use std::sync::atomic::Ordering;

use super::acp::{Blocked, Bridge};
use super::acp_inbound::{blocked_from_locator, permission_id, permission_locator, valid_call};
use super::update_relay::UpdateRelay;
use crate::{ConversationUpdate, PermissionDecision, PermissionLocator, PermissionRequest};

impl Bridge {
    pub(super) async fn blocked_on(
        &self,
        frame: &serde_json::Value,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        let Some(params) = frame.get("params") else {
            return Ok(());
        };
        let Ok(request) = super::runtime_decode::decode::<crate::RequestPermissionRequest>(
            params,
            self.json_pools.is_some(),
        ) else {
            tracing::warn!("ACP permission request has an unreadable shape");
            return Ok(());
        };
        let session = request.session_id.0.to_string();
        if self.session().await.as_deref() != Ok(session.as_str()) {
            tracing::warn!("discarding permission request for another conversation");
            self.refuse_invalid_call(frame).await;
            return Ok(());
        }
        let Some(call) = frame.get("id").cloned().filter(valid_call) else {
            tracing::warn!("ACP permission request has an invalid correlation id");
            return Ok(());
        };
        if self.permission_state.is_some() {
            if let Err(error) = super::permission_state::validate(&request.options) {
                self.refuse_permission_capacity(&call).await;
                return Err(error);
            }
        }
        let locator = permission_locator(&session, &call, &request.options);
        let id = permission_id(&locator);
        let owns_prompt_waiter = self.owned_prompt_waiters.load(Ordering::SeqCst) > 0;
        if let Some(state) = &self.permission_state {
            let entry =
                state
                    .lock()
                    .await
                    .remember(&id, &call, &request.options, owns_prompt_waiter);
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    self.refuse_permission_capacity(&call).await;
                    return Err(error);
                }
            };
            if let Some(answer) = entry.answer() {
                self.send(&answer).await?;
                return Ok(());
            }
        } else {
            if let Some(answer) = self.answered.lock().await.get(&id).cloned() {
                let _ = self.send(&answer).await;
                return Ok(());
            }
            self.blocked.lock().await.insert(
                id.clone(),
                Blocked {
                    call,
                    options: request.options.clone(),
                    owns_prompt_waiter,
                },
            );
        }
        self.route_update(
            &session,
            ConversationUpdate::Asked(PermissionRequest {
                id,
                locator: Some(locator),
                request,
            }),
            updates,
        )
        .await;
        Ok(())
    }

    async fn refuse_permission_capacity(&self, call: &serde_json::Value) {
        let frame = serde_json::json!({
            "jsonrpc":"2.0", "id":call, "error":{
                "code":-32002, "message":"runtime permission retention refused"
            }
        });
        // A peer that does not read cannot keep policy refusal waiting on the
        // sink. Timeout still propagates the original refusal without a receipt.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), self.send(&frame)).await;
    }

    pub(super) async fn bounded_permission_answer(
        &self,
        id: &str,
        locator: Option<&PermissionLocator>,
        session: &str,
        decision: PermissionDecision,
    ) -> crate::Result<()> {
        // Validate supplied authority even if an answer is already retained.
        let restored = match locator {
            Some(locator) => {
                if locator.as_str().len() > 128 * 1024 {
                    return Err(crate::AdapterError::Rejected(
                        "permission locator exceeds its bound".into(),
                    ));
                }
                Some(blocked_from_locator(id, locator, session)?)
            }
            None => None,
        };
        let state = self
            .permission_state
            .as_ref()
            .expect("enabled permission policy");
        let entry = {
            let mut state = state.lock().await;
            match state.get(id) {
                Some(entry) => entry,
                None => {
                    let restored = restored.as_ref().ok_or_else(|| {
                        crate::AdapterError::Rejected(
                            "nothing is waiting for this permission answer".into(),
                        )
                    })?;
                    state.remember(
                        id,
                        &restored.call,
                        &restored.options,
                        restored.owns_prompt_waiter,
                    )?
                }
            }
        };
        entry.select(decision);
        if locator.is_some() && !entry.owns_prompt_waiter {
            self.orphaned_prompt_response.store(true, Ordering::SeqCst);
        }
        // The entry and its reservation survive uncertainty, cancellation and
        // conflicting retries. Only the attachment's final owner releases it.
        self.send(&entry.answer().expect("selected permission answer"))
            .await
    }
}
