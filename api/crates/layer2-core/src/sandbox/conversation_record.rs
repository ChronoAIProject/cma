use super::*;

use crate::seconds_from_rfc3339;
use sha2::{Digest, Sha256};

use crate::sandbox::conversation::PendingAgentUpdate;
use crate::sandbox::conversation_fold::{fold_update, FoldedConversationUpdate};
use crate::sandbox::store::AgentConversationMutation;
use crate::sandbox::Agent;

impl DurableAgentUpdateRecorder {
    pub(super) async fn record_with_reopen(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
        guard: Option<&crate::sandbox::store::AgentReopenGuard>,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        if runtime_generation == 0 {
            return Err(AgentConversationError::StaleGeneration);
        }
        if seconds_from_rfc3339(&appended_at).is_none() {
            return Err(AgentConversationError::InvalidState(
                "conversation update timestamp is invalid",
            ));
        }
        let (source, update) = match update {
            ConversationUpdate::RuntimeOutput { receipt, update } => {
                (Some(receipt.source), update.map(|update| *update))
            }
            update => (None, Some(update)),
        };
        let fingerprint = source.as_ref().map_or_else(
            || {
                format!(
                    "{:x}",
                    Sha256::digest(
                        format!("{:?}", update.as_ref().expect("legacy update is present"))
                            .as_bytes()
                    )
                )
            },
            |source| format!("{:x}", Sha256::digest(format!("{source:?}").as_bytes())),
        );
        let update_id = if let Some(evidence) = &assembly.reconciliation {
            uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_OID,
                format!(
                    "cma-reconciliation-v1:{}:{}:{}:{}",
                    id,
                    evidence.runtime_generation,
                    evidence.turn_sequence,
                    evidence.native_turn_id
                )
                .as_bytes(),
            )
            .to_string()
        } else if let Some(pending) = &assembly.pending_update {
            if pending.fingerprint != fingerprint
                || pending.runtime_generation != runtime_generation
            {
                return Err(AgentConversationError::InvalidState(
                    "an uncertain update must be retried exactly",
                ));
            }
            pending.id.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        for _ in 0..MAX_CONVERSATION_COMMIT_RETRIES {
            let persisted = AgentStore::get_agent(self.store.as_ref(), id)
                .await
                .map_err(store_error)?
                .ok_or(AgentConversationError::NotFound)?;
            let current = match guard {
                Some(guard) => guard
                    .project(&persisted)
                    .map_err(|_| AgentConversationError::StaleGeneration)?,
                None => persisted.clone(),
            };
            if current.conversation.runtime_generation != Some(runtime_generation) {
                return Err(AgentConversationError::StaleGeneration);
            }
            let previous = self
                .store
                .agent_message_checkpoint(id)
                .await
                .map_err(store_error)?;
            if guard.is_none()
                && previous
                    .as_ref()
                    .is_some_and(|checkpoint| checkpoint.runtime_generation > runtime_generation)
            {
                return Err(AgentConversationError::StaleGeneration);
            }
            let recorded = previous
                .as_ref()
                .filter(|checkpoint| checkpoint.update_id == update_id);
            if recorded.is_none() {
                if let Some(evidence) = assembly.reconciliation.as_ref() {
                    evidence.validate_against(&current)?;
                }
            }
            let completed_replay = self
                .completed_prompt_replay(&current, update.as_ref())
                .await
                .map_err(store_error)?;
            let output_replay = if let Some(source) = &source {
                if completed_replay {
                    true
                } else if is_identity_terminal_for_current(&current, update.as_ref()) {
                    // An old observer may have ACKed the same cursor while
                    // treating an identity-bearing terminal as quiet. Let the
                    // current observer apply its verified terminal meaning.
                    false
                } else {
                    output_replayed(source, &current, previous.as_ref())?
                }
            } else {
                completed_replay
            };
            let replayed = output_replay;
            if let Some(recorded) = recorded {
                if recorded.update_fingerprint != fingerprint {
                    return Err(AgentConversationError::Corrupt(
                        "checkpoint update identity conflict".into(),
                    ));
                }
            }
            let mut recovered = match checkpoint::recover_assembly(
                self.store.as_ref(),
                &current,
                previous.as_ref(),
            )
            .await
            {
                Ok(recovered) => recovered,
                Err(error) => {
                    // Aggregate and checkpoint reads are separate. A concurrent
                    // append may invalidate their apparent anchor relationship.
                    let refreshed = AgentStore::get_agent(self.store.as_ref(), id)
                        .await
                        .map_err(store_error)?;
                    if refreshed.as_ref() != Some(&persisted) {
                        continue;
                    }
                    return Err(error);
                }
            };
            if let Some(evidence) = assembly.reconciliation.as_ref() {
                if evidence.final_answer_durable {
                    recovered.content.clear();
                    recovered.message_id = None;
                } else if recovered.content != evidence.final_answer {
                    if !evidence.final_answer.starts_with(&recovered.content) {
                        return Err(AgentConversationError::InvalidState(
                            "native completion answer does not extend the durable prefix",
                        ));
                    }
                    recovered.content = evidence.final_answer.clone();
                    recovered.prompt_evidence = true;
                }
            }
            if replayed && recorded.is_none() {
                if completed_replay {
                    let mut checkpoint = previous.clone().unwrap_or(AgentMessageCheckpoint {
                        version: 1,
                        update_id: update_id.clone(),
                        update_fingerprint: fingerprint.clone(),
                        native_conversation_id: current
                            .conversation
                            .conversation_id
                            .clone()
                            .ok_or(AgentConversationError::InvalidState(
                                "conversation identity is missing",
                            ))?,
                        runtime_generation,
                        active_turn: current.conversation.active_turn,
                        after_seq: current.conversation.last_seq,
                        assembly: recovered.clone(),
                        outcome: AgentUpdateOutcome::quiet(),
                        output: None,
                    });
                    checkpoint.update_id = update_id.clone();
                    checkpoint.update_fingerprint = fingerprint.clone();
                    checkpoint.after_seq = current.conversation.last_seq;
                    checkpoint.active_turn = current.conversation.active_turn;
                    checkpoint.assembly = recovered.clone();
                    checkpoint.outcome = AgentUpdateOutcome::quiet();
                    checkpoint.output = source.clone();
                    checkpoint.validate().map_err(store_error)?;
                    match self
                        .commit_output(persisted.clone(), previous.clone(), checkpoint, None, guard)
                        .await
                    {
                        Ok(_) => {}
                        Err(SandboxStoreError::Conflict) => continue,
                        Err(error) => return Err(store_error(error)),
                    }
                    *assembly = recovered;
                    return Ok(AgentUpdateOutcome::quiet());
                }
                // A reconnect may parse an old terminal as a no-op after the turn closed.
                // The immutable source position is the receipt, independent of that parsing.
                if let Some(reanchored) = previous
                    .as_ref()
                    .and_then(|checkpoint| checkpoint.reanchored(&current))
                {
                    match self
                        .commit_output(persisted.clone(), previous.clone(), reanchored, None, guard)
                        .await
                    {
                        Ok(_) => {}
                        Err(SandboxStoreError::Conflict) => continue,
                        Err(error) => return Err(store_error(error)),
                    }
                }
                if guard.is_some() {
                    let checkpoint = previous
                        .clone()
                        .ok_or(AgentConversationError::StaleGeneration)?;
                    self.commit_output(
                        persisted.clone(),
                        previous.clone(),
                        checkpoint,
                        None,
                        guard,
                    )
                    .await
                    .map_err(store_error)?;
                }
                *assembly = recovered;
                return Ok(AgentUpdateOutcome::quiet());
            }
            if let Some(recorded) = recorded {
                if let Some(reanchored) = recorded.reanchored(&current) {
                    // An exact receipt retry must also persist a verified repair.
                    // Otherwise a later control write still sees the obsolete anchor.
                    match self
                        .commit_output(persisted.clone(), previous.clone(), reanchored, None, guard)
                        .await
                    {
                        Ok(_) => {}
                        Err(SandboxStoreError::Conflict) => continue,
                        Err(error) => return Err(store_error(error)),
                    }
                }
                if guard.is_some() {
                    self.commit_output(
                        persisted.clone(),
                        previous.clone(),
                        recorded.clone(),
                        None,
                        guard,
                    )
                    .await
                    .map_err(store_error)?;
                }
                *assembly = recovered;
                return Ok(recorded.outcome.clone());
            }
            if let Some(pending) = &assembly.pending_update {
                if pending.active_turn != current.conversation.active_turn
                    || pending.after_seq != current.conversation.last_seq
                {
                    return Err(AgentConversationError::Unavailable(
                        "uncertain update receipt is no longer the current checkpoint".into(),
                    ));
                }
            }
            let folded = match &update {
                Some(update) => fold_update(
                    &current,
                    &recovered,
                    update.clone(),
                    &appended_at,
                    self.writer_policy.allows_writer(
                        &current.owner,
                        crate::sandbox::task_experience::TaskExperienceWriter::ToolOutput,
                    ),
                )?,
                None => FoldedConversationUpdate::Quiet(Box::new(recovered.clone())),
            };
            let (next_agent, mut next_assembly, logs, turn_closed) = match folded {
                FoldedConversationUpdate::Quiet(next) => {
                    (current.clone(), *next, Vec::new(), false)
                }
                FoldedConversationUpdate::Changed(change) => (
                    change.agent,
                    change.assembly,
                    change.logs,
                    change.turn_closed,
                ),
            };
            let fragment = update.as_ref().and_then(|update| {
                live::accepted_fragment(
                    &current,
                    &recovered,
                    &mut next_assembly,
                    update,
                    next_agent.conversation.last_seq,
                )
            });
            // Thoughts remain ephemeral, and an unchanged bounded message needs no write.
            // Accepted assistant text is checkpointed before it can be published live.
            let durable_assembly_changed = next_assembly.message_id != recovered.message_id
                || next_assembly.content != recovered.content
                || next_assembly.prompt_evidence != recovered.prompt_evidence;
            let recovered_anchor = !recovered.is_empty()
                && previous
                    .as_ref()
                    .is_some_and(|checkpoint| !checkpoint.matches(&current));
            if source.is_none() && logs.is_empty() && !durable_assembly_changed && !recovered_anchor
            {
                *assembly = next_assembly;
                self.live.publish(fragment);
                return Ok(AgentUpdateOutcome::quiet());
            }
            let outcome = AgentUpdateOutcome {
                appended: logs
                    .iter()
                    .enumerate()
                    .map(|(index, kind)| crate::sandbox::AgentLogEntry {
                        agent_id: id.clone(),
                        seq: current.conversation.last_seq + index as u64 + 1,
                        appended_at: appended_at.clone(),
                        kind: kind.clone(),
                    })
                    .collect(),
                turn_closed,
            };
            let checkpoint = AgentMessageCheckpoint {
                version: 1,
                update_id: update_id.clone(),
                update_fingerprint: fingerprint.clone(),
                native_conversation_id: current.conversation.conversation_id.clone().ok_or(
                    AgentConversationError::InvalidState("conversation identity is missing"),
                )?,
                runtime_generation,
                active_turn: next_agent.conversation.active_turn,
                after_seq: next_agent.conversation.last_seq,
                assembly: next_assembly.clone(),
                outcome,
                output: source.clone().or_else(|| {
                    previous
                        .as_ref()
                        .filter(|checkpoint| {
                            checkpoint.native_conversation_id
                                == current
                                    .conversation
                                    .conversation_id
                                    .as_deref()
                                    .unwrap_or_default()
                                && checkpoint.runtime_generation == runtime_generation
                        })
                        .and_then(|checkpoint| checkpoint.output.clone())
                }),
            };
            checkpoint.validate().map_err(store_error)?;
            let mutation = (!logs.is_empty()).then(|| AgentConversationMutation {
                settings_fence: None,
                command_claim: None,
                execution_fence: None,
                trigger_execution_fence: None,
                admission_guard: None,
                reconciliation_fence: assembly.reconciliation.as_ref().map(|evidence| {
                    crate::sandbox::store::AgentReconciliationFence {
                        sandbox_id: current.sandbox_id.clone(),
                        sandbox_revision: evidence.sandbox_revision,
                        sandbox_status: evidence.sandbox_status,
                    }
                }),
                expected_agent_revision: current.revision,
                expected_last_seq: current.conversation.last_seq,
                runtime_generation,
                agent: next_agent,
                appended_at: appended_at.clone(),
                agent_logs: logs,
            });
            match self
                .commit_output(persisted.clone(), previous, checkpoint, mutation, guard)
                .await
            {
                Ok(outcome) => {
                    *assembly = next_assembly;
                    self.live.publish(fragment);
                    return Ok(outcome);
                }
                Err(SandboxStoreError::Conflict) => {}
                Err(SandboxStoreError::SequenceConflict) => {
                    return Err(AgentConversationError::Corrupt(
                        "agent log sequence is inconsistent".into(),
                    ));
                }
                Err(error @ SandboxStoreError::Unavailable(_)) => {
                    assembly.pending_update = Some(Box::new(PendingAgentUpdate {
                        id: update_id.clone(),
                        fingerprint: fingerprint.clone(),
                        runtime_generation,
                        active_turn: current.conversation.active_turn,
                        after_seq: current.conversation.last_seq,
                    }));
                    return Err(store_error(error));
                }
                Err(error) => return Err(store_error(error)),
            }
        }
        Err(AgentConversationError::Unavailable(
            "conversation changed during every commit attempt".into(),
        ))
    }

    async fn completed_prompt_replay(
        &self,
        current: &Agent,
        update: Option<&ConversationUpdate>,
    ) -> Result<bool, SandboxStoreError> {
        let prompt_id = match update {
            Some(ConversationUpdate::TurnEndedForPrompt { prompt_id, .. })
            | Some(ConversationUpdate::FailedForPrompt { prompt_id, .. }) => prompt_id,
            _ => return Ok(false),
        };
        let Some(conversation) = current.conversation.conversation_id.as_deref() else {
            return Ok(false);
        };
        let Some(generation) = current.conversation.runtime_generation else {
            return Ok(false);
        };
        let after = current
            .conversation
            .last_seq
            .saturating_sub(u64::from(crate::sandbox::store::MAX_PAGE_SIZE));
        let logs = self
            .store
            .agent_log(
                &current.id,
                after,
                current.conversation.last_seq.saturating_sub(after) as u16,
            )
            .await?;
        Ok(logs.iter().any(|entry| {
            matches!(
                &entry.kind,
                crate::sandbox::AgentLogKind::TurnCompleted { turn_sequence }
                    if *prompt_id
                        == layer3_adapter::PromptIdentity::new(conversation, generation, *turn_sequence)
            )
        }))
    }
}

fn is_identity_terminal_for_current(current: &Agent, update: Option<&ConversationUpdate>) -> bool {
    let Some(update) = update else { return false };
    let Some(conversation) = current.conversation.conversation_id.as_deref() else {
        return false;
    };
    let Some(generation) = current.conversation.runtime_generation else {
        return false;
    };
    let Some(turn) = current.conversation.active_turn else {
        return false;
    };
    let expected = layer3_adapter::PromptIdentity::new(conversation, generation, turn);
    matches!(
        update,
        ConversationUpdate::TurnEndedForPrompt { prompt_id, .. }
            | ConversationUpdate::FailedForPrompt { prompt_id, .. }
            if prompt_id == &expected
    )
}
