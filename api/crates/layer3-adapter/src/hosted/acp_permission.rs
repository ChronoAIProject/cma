use std::sync::atomic::Ordering;

use super::acp::{Blocked, Bridge};
use super::acp_inbound::{blocked_from_locator, permission_id, permission_locator};
use super::update_relay::UpdateRelay;
use crate::{ConversationUpdate, PermissionDecision, PermissionLocator, PermissionRequest};

impl Bridge {
    pub(super) fn prepare_permission(
        &self,
        frame: &serde_json::Value,
        policy: &crate::ingress_budget::IngressOwnershipPolicy,
    ) -> crate::Result<Option<super::acp::ingress::PreparedUpdate>> {
        let Some(params) = frame.get("params") else {
            return Ok(None);
        };
        let Some(call) = frame
            .get("id")
            .filter(|call| super::acp_correlation::valid_call_with_policy(call, true))
        else {
            return Ok(None);
        };
        let options = params.get("options").and_then(serde_json::Value::as_array);
        let count = options.map_or(0, Vec::len);
        let strings = options
            .into_iter()
            .flatten()
            .try_fold(
                params
                    .get("sessionId")
                    .and_then(serde_json::Value::as_str)
                    .map_or(0, str::len),
                |sum, option| {
                    sum.checked_add(
                        option
                            .get("optionId")
                            .and_then(serde_json::Value::as_str)
                            .map_or(0, str::len),
                    )
                },
            )
            .ok_or_else(permission_layout)?;
        // JSON escaping uses at most six bytes per UTF-8 byte. The fixed keys,
        // kind strings and punctuation use fewer than 128 bytes per option;
        // the already validated serialized correlation uses at most 256.
        let encoded = strings
            .checked_mul(6)
            .and_then(|bytes| count.checked_add(1)?.checked_mul(128)?.checked_add(bytes))
            .and_then(|bytes| bytes.checked_add(256))
            .ok_or_else(permission_layout)?;
        let retained = encoded
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(64))
            .ok_or_else(permission_layout)?;
        let temporary = count
            .checked_mul(3)
            .map(|slots| slots.max(4))
            .and_then(|slots| slots.checked_mul(size_of::<String>() + size_of::<usize>()))
            .and_then(|bytes| bytes.checked_add(strings))
            .and_then(|bytes| bytes.checked_add(encoded.checked_mul(3)?))
            .ok_or_else(permission_layout)?;
        // Typed decoding finishes before locator serialization starts. Its
        // retained request has a separate allowance throughout both phases.
        let admission = super::acp::ingress::typed::TypedAdmission::with_alternative_work::<
            ConversationUpdate,
        >(params, policy, retained, temporary)?;
        let result = admission
            .initialize(|| {
                let request: crate::RequestPermissionRequest =
                    super::runtime_decode::decode(params, true)?;
                let locator = permission_locator(&request.session_id.0, call, &request.options);
                let id = permission_id(&locator);
                Ok::<_, super::runtime_decode::Error>(ConversationUpdate::Asked(
                    PermissionRequest {
                        id,
                        locator: Some(locator),
                        request,
                    },
                ))
            })
            .transpose();
        Ok(Some(match result {
            Ok(update) => Ok(crate::RuntimeOutputUpdate::from_admitted(
                update.into_shared()?,
            )),
            Err(error) => Err(error.into_shared()?),
        }))
    }

    async fn admitted_permission(
        &self,
        frame: &super::retained_json::JsonFrame,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        let Some(prepared) = frame.prepared_update() else {
            return Ok(());
        };
        let update = match prepared {
            Ok(update) => update,
            Err(_) => {
                tracing::warn!("ACP permission request has an unreadable shape");
                return Ok(());
            }
        };
        let ConversationUpdate::Asked(permission) = &*update else {
            return Err(permission_layout());
        };
        let session = permission.request.session_id.0.as_ref();
        if !self.matches_session(session).await {
            self.refuse_invalid_call(frame).await;
            return Ok(());
        }
        let call = frame.get("id").ok_or_else(permission_layout)?;
        let state = self
            .permission_state
            .as_ref()
            .ok_or_else(permission_layout)?;
        let entry = state.lock().await.remember(
            &permission.id,
            call,
            &permission.request.options,
            self.owned_prompt_waiters.load(Ordering::SeqCst) > 0,
        );
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                self.refuse_permission_capacity(call).await;
                return Err(error);
            }
        };
        if self.replay_permission(&entry).await? {
            return Ok(());
        }
        self.route_update(
            session,
            ConversationUpdate::RetainedEffect(update.clone()),
            updates,
        )
        .await?;
        Ok(())
    }

    pub(super) async fn blocked_on(
        &self,
        frame: &super::retained_json::JsonFrame,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        if self.ingress.is_some() {
            return self.admitted_permission(frame, updates).await;
        }
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
        let Some(call) = super::acp_correlation::permission_call(frame, self.json_pools.is_some())
        else {
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
        .await?;
        Ok(())
    }

    async fn refuse_permission_capacity(&self, call: &serde_json::Value) {
        let storage = match &self.ingress {
            Some(policy) => match super::permission_state::reply_plan()
                .and_then(|plan| policy.reserve(crate::ingress_budget::IngressClass::Control, plan))
            {
                Ok(storage) => Some(storage),
                Err(_) => {
                    tracing::warn!("permission refusal could not admit control storage");
                    return;
                }
            },
            None => None,
        };
        let frame = serde_json::json!({
            "jsonrpc":"2.0", "id":call, "error":{
                "code":-32002, "message":"runtime permission retention refused"
            }
        });
        // A peer that does not read cannot keep policy refusal waiting on the
        // sink. Timeout still propagates the original refusal without a receipt.
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            self.send_control_frame(&frame),
        )
        .await;
        drop(frame);
        drop(storage);
    }

    async fn replay_permission(
        &self,
        entry: &super::permission_state::Entry,
    ) -> crate::Result<bool> {
        match &self.ingress {
            Some(policy) => {
                let Some(answer) = entry.admitted_answer(policy)? else {
                    return Ok(false);
                };
                self.send_control_frame(&answer).await?;
            }
            None => {
                let Some(answer) = entry.answer() else {
                    return Ok(false);
                };
                self.send(&answer).await?;
            }
        }
        Ok(true)
    }

    pub(super) async fn bounded_permission_answer(
        &self,
        id: &str,
        locator: Option<&PermissionLocator>,
        session: &str,
        decision: PermissionDecision,
    ) -> crate::Result<()> {
        // Validate supplied authority even if an answer is already retained.
        let _restored_storage = match (&self.ingress, locator) {
            (Some(_), Some(locator)) => {
                if locator.as_str().len() > 128 * 1024 {
                    return Err(permission_layout());
                }
                // This bounded parser/tree allowance also covers the locator's
                // smaller typed containers and String-to-Arc conversion. Keep
                // it until both restored choices and their temporary copies drop.
                Some(
                    self.json_pools
                        .as_ref()
                        .ok_or_else(permission_layout)?
                        .admit(locator.as_str().as_bytes())?,
                )
            }
            _ => None,
        };
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
        self.replay_permission(&entry).await.map(|_| ())
    }
}

fn permission_layout() -> crate::AdapterError {
    crate::AdapterError::Rejected("permission ownership layout is invalid".into())
}
