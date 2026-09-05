use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::acp::{Bridge, Outcome, TURN_DEADLINE};
use super::acp_inbound::{blocked_from_locator, outcome_for};
use super::update_relay::UpdateRelay;
use crate::{ConversationInput, ConversationUpdate, PromptResponse};

impl Bridge {
    pub(super) async fn drive(
        &self,
        input: ConversationInput,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        let session = self.session().await?;
        match input {
            ConversationInput::Prompt(content) => {
                self.answered.lock().await.clear();
                self.owned_prompt_waiters.fetch_add(1, Ordering::SeqCst);
                let params = serde_json::json!({
                    "sessionId": session,
                    "prompt": content,
                });
                match self.begin_prompt(params).await {
                    Ok(waiting) => {
                        let answered_permissions = Arc::clone(&self.answered);
                        let owned_prompt_waiters = Arc::clone(&self.owned_prompt_waiters);
                        let updates = updates.clone();
                        tokio::spawn(async move {
                            let answered = tokio::time::timeout(TURN_DEADLINE, waiting).await;
                            let previous = owned_prompt_waiters.fetch_sub(1, Ordering::SeqCst);
                            debug_assert!(previous > 0, "prompt waiter is released once");
                            answered_permissions.lock().await.clear();
                            let update = match answered {
                                Err(_) => Some(scoped_turn_outcome(
                                    Outcome::Failed {
                                        message: "agent did not answer this turn".into(),
                                        capability: None,
                                    },
                                    false,
                                )),
                                Ok(Ok(_)) => None,
                                Ok(Err(_)) => Some(scoped_turn_outcome(
                                    Outcome::Failed {
                                        message: "agent stopped answering before the turn finished"
                                            .into(),
                                        capability: None,
                                    },
                                    false,
                                )),
                            };
                            if let Some(update) = update {
                                updates.send(update);
                            }
                        });
                        Ok(())
                    }
                    Err(error) => {
                        let previous = self.owned_prompt_waiters.fetch_sub(1, Ordering::SeqCst);
                        debug_assert!(previous > 0, "failed prompt releases its waiter");
                        updates.send(scoped_turn_outcome(
                            Outcome::Failed {
                                message: format!("prompt could not be sent to the agent: {error}"),
                                capability: None,
                            },
                            false,
                        ));
                        Err(error)
                    }
                }
            }
            ConversationInput::PermissionAnswer {
                id,
                locator,
                decision,
            } => {
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

pub(super) fn scoped_turn_outcome(outcome: Outcome, prompt_evidence: bool) -> ConversationUpdate {
    let update = outcome.into_update();
    if prompt_evidence {
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
    match serde_json::from_value::<PromptResponse>(result.clone()) {
        Ok(response) => Outcome::Ended(response),
        Err(error) => {
            tracing::warn!(%error, "turn ended in a newer ACP shape");
            Outcome::Ended(PromptResponse::new(crate::TurnStopReason::EndTurn))
        }
    }
}
