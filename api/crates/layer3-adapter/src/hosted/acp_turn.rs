use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::acp::{Bridge, Outcome};
use super::acp_inbound::{blocked_from_locator, outcome_for};
use super::update_relay::UpdateRelay;
use crate::{ConversationInput, ConversationUpdate, PromptResponse};

impl Bridge {
    pub(super) async fn drive(
        &self,
        input: ConversationInput,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        let recovering = self.recovering_context.load(Ordering::SeqCst);
        if recovering != 0
            && !matches!(&input,
            ConversationInput::RefreshContext { operation_sequence, .. } if *operation_sequence == recovering)
        {
            return Err(crate::AdapterError::OperationIdentityConflict(
                "admitted context recovery must complete before other input".into(),
            ));
        }
        let session = self.session().await?;
        match input {
            ConversationInput::Configure {
                model,
                reasoning_effort,
                permission,
                system_prompt,
            } => {
                self.configure_settings(
                    &session,
                    &model,
                    reasoning_effort,
                    permission,
                    system_prompt.as_deref(),
                )
                .await
            }
            ConversationInput::RefreshContext {
                service_origin,
                operation_sequence,
                readiness_receipt,
                previous_guidance,
                working_directory,
                guidance,
                model,
                reasoning_effort,
                permission,
            } => {
                self.refresh_context(
                    &session,
                    operation_sequence,
                    &readiness_receipt,
                    &guidance,
                    &previous_guidance,
                    &model,
                    reasoning_effort,
                    permission,
                    working_directory.as_deref(),
                    service_origin.as_deref(),
                )
                .await?;
                self.recovering_context.store(0, Ordering::SeqCst);
                Ok(())
            }
            ConversationInput::Steer(content) => {
                // Probe before mutation so an older retained runtime refuses
                // steering without creating a competing prompt or waiter.
                let support = self.call("cma/steer", serde_json::json!({ "action": "support" }))
                    .await.map_err(|error| match error {
                        crate::AdapterError::Rejected(_) => crate::AdapterError::Unsupported(
                            "this runtime cannot safely steer an active turn; update the Sandbox when idle".into()),
                        other => other,
                    })?;
                if support
                    .get("supported")
                    .and_then(serde_json::Value::as_bool)
                    != Some(true)
                    || support.get("version").and_then(serde_json::Value::as_u64) != Some(1)
                {
                    return Err(crate::AdapterError::Unsupported(
                        "runtime steering contract is unavailable".into(),
                    ));
                }
                let result = self
                    .call(
                        "cma/steer",
                        serde_json::json!({
                            "sessionId": session, "prompt": content,
                        }),
                    )
                    .await?;
                if result.get("injected").and_then(serde_json::Value::as_bool) != Some(true)
                    || result.get("sessionId").and_then(serde_json::Value::as_str)
                        != Some(session.as_str())
                {
                    return Err(crate::AdapterError::Unavailable(
                        "runtime steering was not confirmed".into(),
                    ));
                }
                Ok(())
            }
            ConversationInput::Prompt(content) => {
                self.drive_prompt(session, content, None, updates).await
            }
            ConversationInput::RecoverablePrompt { content, prompt_id } => {
                self.drive_prompt(session, content, Some(prompt_id), updates)
                    .await
            }
            ConversationInput::PermissionAnswer {
                id,
                locator,
                decision,
            } => {
                if self.permission_state.is_some() {
                    return self
                        .bounded_permission_answer(&id, locator.as_ref(), &session, decision)
                        .await;
                }
                let restored = match locator.as_ref() {
                    Some(locator) => Some(blocked_from_locator(&id, locator, &session)?),
                    None => None,
                };
                if let Some(answer) = self.answered.lock().await.get(&id).cloned() {
                    return self.send(&answer).await;
                }
                let durable_route = restored.is_some();
                let held = match restored {
                    Some(restored) => self.blocked.lock().await.remove(&id).unwrap_or(restored),
                    None => self.blocked.lock().await.remove(&id).ok_or_else(|| {
                        crate::AdapterError::Rejected(
                            "nothing is waiting for this permission answer".into(),
                        )
                    })?,
                };
                let outcome = outcome_for(decision, &held.options);
                let answer = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": held.call,
                    "result": { "outcome": outcome },
                });
                let orphaned_prompt = durable_route && !held.owns_prompt_waiter;
                if orphaned_prompt {
                    self.orphaned_prompt_response.store(true, Ordering::SeqCst);
                }
                self.answered
                    .lock()
                    .await
                    .insert(id.clone(), answer.clone());
                if let Err(error) = self.send(&answer).await {
                    self.answered.lock().await.remove(&id);
                    if orphaned_prompt {
                        self.orphaned_prompt_response.store(false, Ordering::SeqCst);
                    }
                    return Err(error);
                }
                Ok(())
            }
            ConversationInput::Cancel => {
                self.send(&serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/cancel",
                    "params": { "sessionId": session },
                }))
                .await
            }
        }
    }

    async fn drive_prompt(
        &self,
        session: String,
        content: Vec<crate::ContentBlock>,
        prompt_id: Option<crate::PromptIdentity>,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        self.answered.lock().await.clear();
        self.owned_prompt_waiters.fetch_add(1, Ordering::SeqCst);
        let params = serde_json::json!({
            "sessionId": session,
            "prompt": content,
        });
        let recoverable = prompt_id.is_some();
        let bounded_permissions = self.permission_state.is_some();
        let terminal_identity = prompt_id.clone();
        match self.begin_prompt(params, prompt_id).await {
            Ok(waiting) => {
                let answered_permissions = Arc::clone(&self.answered);
                let owned_prompt_waiters = Arc::clone(&self.owned_prompt_waiters);
                let updates = updates.clone();
                tokio::spawn(async move {
                    // A turn can outlive any API request. The bridge owns the
                    // waiter until an actual provider result or link closure;
                    // elapsed time cannot invent a terminal for a later turn.
                    let answered = waiting.await;
                    let previous = owned_prompt_waiters.fetch_sub(1, Ordering::SeqCst);
                    debug_assert!(previous > 0, "prompt waiter is released once");
                    answered_permissions.lock().await.clear();
                    let update = match answered {
                        Ok(_) => None,
                        // The runtime still owns this identified prompt when
                        // its API attachment closes. Connection recovery must
                        // observe its original result, never invent a failure.
                        Err(_) if recoverable || bounded_permissions => None,
                        Err(_) => Some(scoped_turn_outcome(
                            Outcome::Failed {
                                message: "agent stopped answering before the turn finished".into(),
                                capability: None,
                            },
                            false,
                            terminal_identity.clone(),
                        )),
                    };
                    if let Some(update) = update {
                        updates.send(update).await;
                    }
                });
                Ok(())
            }
            Err(error) => {
                let previous = self.owned_prompt_waiters.fetch_sub(1, Ordering::SeqCst);
                debug_assert!(previous > 0, "failed prompt releases its waiter");
                updates
                    .send(scoped_turn_outcome(
                        Outcome::Failed {
                            message: format!("prompt could not be sent to the agent: {error}"),
                            capability: None,
                        },
                        false,
                        terminal_identity,
                    ))
                    .await;
                Err(error)
            }
        }
    }
}

impl Outcome {
    pub(super) fn into_update(self) -> ConversationUpdate {
        match self {
            Outcome::Ended(response) => ConversationUpdate::TurnEnded(response),
            Outcome::Failed {
                message,
                capability,
            } => ConversationUpdate::Failed {
                message,
                capability,
            },
        }
    }
}

pub(super) fn scoped_turn_outcome(
    outcome: Outcome,
    prompt_evidence: bool,
    prompt_id: Option<crate::PromptIdentity>,
) -> ConversationUpdate {
    let update = match (outcome, prompt_id) {
        (Outcome::Ended(response), Some(prompt_id)) => ConversationUpdate::TurnEndedForPrompt {
            prompt_id,
            response,
        },
        (
            Outcome::Failed {
                message,
                capability,
            },
            Some(prompt_id),
        ) => ConversationUpdate::FailedForPrompt {
            prompt_id,
            message,
            capability,
        },
        (outcome, None) => outcome.into_update(),
    };
    if prompt_evidence
        || matches!(
            update,
            ConversationUpdate::TurnEndedForPrompt { .. }
                | ConversationUpdate::FailedForPrompt { .. }
        )
    {
        update
    } else {
        update.without_prompt_evidence()
    }
}

pub(super) fn turn_outcome(frame: serde_json::Value) -> Outcome {
    if let Some(error) = frame.get("error") {
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("agent refused the prompt without saying why")
            .to_string();
        if let Some(failure) = super::acp_credit_failure::classify(&message) {
            return failure;
        }
        return Outcome::Failed {
            message,
            capability: error
                .get("data")
                .and_then(|data| data.get("capability"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        };
    }
    let Some(result) = frame.get("result") else {
        return Outcome::Failed {
            message: "agent answered with neither a result nor an error".into(),
            capability: None,
        };
    };
    if let Some(failure) = result.pointer("/_meta/jetbrains/air/sessionFailure") {
        if !failure.is_null()
            && failure.get("severity").and_then(serde_json::Value::as_str) != Some("warning")
        {
            if let Some(failure) = failure
                .get("title")
                .and_then(serde_json::Value::as_str)
                .and_then(super::acp_credit_failure::classify)
            {
                return failure;
            }
            return Outcome::Failed {
                message: failure
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .filter(|title| !title.trim().is_empty())
                    .map(|title| title.chars().take(1024).collect())
                    .unwrap_or_else(|| "The model could not complete this turn".into()),
                capability: None,
            };
        }
    }
    match serde_json::from_value::<PromptResponse>(result.clone()) {
        Ok(response) => Outcome::Ended(response),
        Err(error) => {
            tracing::warn!(%error, "turn ended in a newer ACP shape");
            Outcome::Failed {
                message: "The agent returned an unsupported turn result".into(),
                capability: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_failure_metadata_overrides_end_turn() {
        let frame = serde_json::json!({"result": {
            "stopReason": "end_turn",
            "_meta": {"jetbrains": {"air": {"version": 1, "sessionFailure": {
                "id": "turn-1:error", "revision": 1, "severity": "error",
                "category": "connection", "title": "The model stream disconnected", "actions": ["retry"]
            }}}}
        }});
        assert!(matches!(turn_outcome(frame), Outcome::Failed {message, ..}
            if message == "The model stream disconnected"));
    }

    #[test]
    fn recovered_retry_warning_does_not_fail_a_completed_turn() {
        let frame = serde_json::json!({"result": {
            "stopReason": "end_turn",
            "_meta": {"jetbrains": {"air": {"version": 1, "sessionFailure": {
                "severity": "warning", "title": "Retrying connection"
            }}}}
        }});
        assert!(matches!(turn_outcome(frame), Outcome::Ended(_)));
        assert!(matches!(
            turn_outcome(serde_json::json!({"result": {"stopReason": "end_turn"}})),
            Outcome::Ended(_)
        ));
    }

    #[test]
    fn malformed_result_does_not_report_success() {
        for result in [
            serde_json::json!({}),
            serde_json::json!({"stopReason": "new_unknown_reason"}),
        ] {
            assert!(matches!(
                turn_outcome(serde_json::json!({"result": result})),
                Outcome::Failed { .. }
            ));
        }
    }
}
