//! Running one turn, and reading how it ended.
//!
//! Split from `acp.rs` so both stay within the project's file-size limit, on a
//! division that is real: that file owns the link and this one owns what is
//! said across it. A turn's ending is the part a person sees as the agent
//! stopping, and it is worth being able to read without the socket around it.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::conversation::{ConversationInput, ConversationUpdate, PromptResponse};

use super::acp::{Bridge, Outcome, TURN_DEADLINE};
use super::inbound::{blocked_from_locator, outcome_for};

impl Bridge {
    /// Act on something a caller sent toward the runtime.
    pub(super) async fn drive(
        &self,
        input: ConversationInput,
        updates: &mpsc::Sender<ConversationUpdate>,
    ) -> Result<(), crate::AdapterError> {
        let Some(session) = self.session.lock().await.clone() else {
            return Err(crate::AdapterError::Unavailable(
                "the runtime holds no open conversation".to_string(),
            ));
        };
        match input {
            ConversationInput::Prompt(content) => {
                // Permission request ids are JSON-RPC correlation ids, not
                // conversation-global identities. A later turn may reuse one;
                // retaining an earlier answer would silently apply the old
                // owner's decision to a new request and grow for the life of
                // the socket.
                self.answered.lock().await.clear();
                // Set before writing the prompt. A runtime can raise its first
                // permission request on the reader task before `begin` returns,
                // and an answer to that request must still know this Bridge
                // owns the eventual prompt response.
                self.owned_prompt_waiters
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // The turn's answer is the turn ending, so it is awaited on its
                // own task: a prompt takes as long as the model takes, and
                // blocking here would stop the caller cancelling it.
                let params = serde_json::json!({ "sessionId": session, "prompt": content });
                let updates = updates.clone();
                // Not through `call`, which is otherwise the way to ask this
                // bridge something: that one waits under a deadline meant for a
                // question with a quick answer, and a prompt takes as long as
                // the model takes.
                match self.begin("session/prompt", params).await {
                    Ok(waiting) => {
                        let answered_permissions = Arc::clone(&self.answered);
                        let owned_prompt_waiters = Arc::clone(&self.owned_prompt_waiters);
                        tokio::spawn(async move {
                            let answered = tokio::time::timeout(TURN_DEADLINE, waiting).await;
                            let previous = owned_prompt_waiters
                                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                            debug_assert!(previous > 0, "a prompt waiter is released once");
                            // A response ends the scope in which permission
                            // request ids and their duplicate answers are live.
                            answered_permissions.lock().await.clear();
                            let outcome = match answered {
                                // Nothing at all, for an hour. Said rather
                                // than left running: a turn nobody will ever
                                // answer looks exactly like a turn in
                                // progress, and only one of them is worth
                                // waiting for.
                                Err(_) => Outcome::Failed {
                                    message: "the agent did not answer this turn".to_string(),
                                    capability: None,
                                },
                                Ok(Ok(frame)) => turn_outcome(frame),
                                // The bridge went away mid-turn. This is a
                                // failure, not an ending: the agent did not
                                // answer, and reporting it as a completed turn
                                // is what made a dead bridge look like an
                                // agent with nothing to say.
                                Ok(Err(_)) => Outcome::Failed {
                                    message: "the agent stopped answering before the turn finished"
                                        .to_string(),
                                    capability: None,
                                },
                            };
                            let _ = updates.send(outcome.into_update()).await;
                        });
                        Ok(())
                    }
                    // A prompt that could not even be sent is a failed turn,
                    // not a silent no-op: the caller is waiting on an answer
                    // that is never coming.
                    Err(e) => {
                        let previous = self
                            .owned_prompt_waiters
                            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        debug_assert!(previous > 0, "a failed prompt releases its waiter");
                        tracing::warn!(error = %e, "could not send a prompt");
                        let _ = updates
                            .send(ConversationUpdate::Failed {
                                message: format!("the prompt could not be sent to the agent: {e}"),
                                capability: None,
                            })
                            .await;
                        Err(e)
                    }
                }
            }
            ConversationInput::PermissionAnswer {
                id,
                locator,
                decision,
            } => {
                let repeated_answer = self.answered.lock().await.get(&id).cloned();
                if let Some(answer) = repeated_answer {
                    return self.send(&answer).await;
                }
                let durable_route = locator.is_some();
                let held = match locator {
                    Some(locator) => match blocked_from_locator(&id, &locator, &session) {
                        Ok(restored) => match self.blocked.lock().await.remove(&id) {
                            Some(live) => Ok(live),
                            None => Ok(restored),
                        },
                        Err(error) => Err(error),
                    },
                    None => self.blocked.lock().await.remove(&id).ok_or_else(|| {
                        crate::AdapterError::Rejected(
                            "nothing is waiting for this permission answer".to_string(),
                        )
                    }),
                };
                let held = match held {
                    Ok(held) => held,
                    Err(error) => {
                        // Answered after the bridge stopped waiting, or with a
                        // locator that cannot safely route this id.
                        tracing::info!(request = %id, "nothing is waiting for this answer any more");
                        return Err(error);
                    }
                };
                let outcome = outcome_for(decision, &held.options);
                let answer = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": held.call,
                    "result": { "outcome": outcome },
                });
                let orphaned_prompt = durable_route && !held.owns_prompt_waiter;
                if orphaned_prompt {
                    // Register before the response is written: a fast runtime
                    // can finish the prompt on the reader task before `send`
                    // returns to this one.
                    self.orphaned_prompt_response
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                // Cache before writing for the same ordering reason. Prompt
                // completion clears the whole turn; inserting afterwards can
                // race that clear and leave a stale decision in the next turn.
                self.answered
                    .lock()
                    .await
                    .insert(id.clone(), answer.clone());
                if let Err(error) = self.send(&answer).await {
                    self.answered.lock().await.remove(&id);
                    if orphaned_prompt {
                        self.orphaned_prompt_response
                            .store(false, std::sync::atomic::Ordering::SeqCst);
                    }
                    return Err(error);
                }
                Ok(())
            }
            ConversationInput::Cancel => {
                let notice = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "session/cancel",
                    "params": { "sessionId": session },
                });
                self.send(&notice).await
            }
        }
    }
}

impl Outcome {
    pub(super) fn into_update(self) -> ConversationUpdate {
        match self {
            Outcome::Ended(ended) => ConversationUpdate::TurnEnded(ended),
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

/// Read how a turn ended out of the bridge's answer.
///
/// The answer is the whole JSON-RPC frame — `inbound` hands the receiver what
/// it read, and `call` is what usually unwraps `result`. This path did not, and
/// deserialized the envelope itself into a `PromptResponse`. That could only
/// fail, so every turn was reported as having ended normally: a refusal, a
/// context limit and a cancellation all arrived as `end_turn`, and so did an
/// error. A turn that failed looked exactly like a turn that finished.
///
/// Three cases, and each is said rather than flattened:
///
/// * an error — the turn did not finish, and reporting an ending would tell
///   the owner their agent had answered when it had not;
/// * a result that reads — the kernel's own stop reason, which is the whole
///   point of the field;
/// * a result that does not — recorded loudly and ended, because a turn that
///   is over must be closed either way or the session sits `running` forever.
pub(super) fn turn_outcome(frame: serde_json::Value) -> Outcome {
    if let Some(error) = frame.get("error") {
        // The agent's own words for what went wrong — which is where "could
        // not reach the model", an upstream 401, and a refused tool all
        // arrive. Carried through rather than flattened to a stop reason,
        // because the message is the only part a person can act on.
        tracing::warn!(%error, "the bridge refused a prompt");
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("the agent refused the prompt without saying why")
            .to_string();
        return Outcome::Failed {
            message,
            capability: error
                .get("data")
                .and_then(|d| d.get("capability"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        };
    }
    let Some(result) = frame.get("result") else {
        tracing::warn!(answer = %frame, "a prompt was answered with neither a result nor an error");
        return Outcome::Failed {
            message: "the agent answered with neither a result nor an error".to_string(),
            capability: None,
        };
    };
    match serde_json::from_value::<PromptResponse>(result.clone()) {
        Ok(ended) => Outcome::Ended(ended),
        Err(e) => {
            // A newer protocol naming a new way for a turn to finish must not
            // make a healthy session look broken, so this stays an ending.
            tracing::warn!(
                error = %e,
                answer = %result,
                "a turn ended in a way this build cannot read; treating it as finished"
            );
            Outcome::Ended(PromptResponse::new(crate::TurnStopReason::EndTurn))
        }
    }
}
