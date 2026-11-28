//! Ordered conversation folding and durable output publication.
use super::coalesce::{self, RecordInput};
use super::output::{is_identity_terminal_for_current, output_source_for_checkpoint, store_error};
use super::{
    budget, checkpoint, live, output, DurableAgentUpdateRecorder, MAX_CONVERSATION_COMMIT_RETRIES,
};
use crate::sandbox::conversation::PendingAgentUpdate;
use crate::sandbox::conversation_fold::{fold_with_output_policy, FoldedConversationUpdate};
use crate::sandbox::store::{
    AgentConversationMutation, AgentMessageCheckpoint, AgentStore, SandboxStoreError,
};
use crate::sandbox::AgentOutputCommit;
use crate::sandbox::{
    AgentConversationError, AgentMessageAssembly, AgentOutputCategory, AgentUpdateOutcome,
};
use crate::{seconds_from_rfc3339, AgentId};

impl DurableAgentUpdateRecorder {
    pub(super) async fn record_with_reopen(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        input: RecordInput<'_>,
        appended_at: String,
        guard: Option<&crate::sandbox::store::AgentReopenGuard>,
    ) -> Result<AgentOutputCommit, AgentConversationError> {
        if runtime_generation == 0 {
            return Err(AgentConversationError::StaleGeneration);
        }
        if seconds_from_rfc3339(&appended_at).is_none() {
            return Err(AgentConversationError::InvalidState(
                "conversation update timestamp is invalid",
            ));
        }
        if !input.coalesce() && !input.flush() && assembly.buffered_output().is_some() {
            return Err(AgentConversationError::InvalidState(
                "flush quiet receipts before immediate recording",
            ));
        }
        let (source, update, shared) = input.parts();
        let fingerprint = output::fingerprint(source, update);
        let update_id = if let Some(pending) = &assembly.pending_update {
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
            let snapshot = self
                .store
                .agent_message_snapshot(id)
                .await
                .map_err(store_error)?
                .ok_or(AgentConversationError::NotFound)?;
            let persisted = snapshot.agent;
            let previous = snapshot.checkpoint;
            let current = match guard {
                Some(guard) => guard
                    .project(&persisted)
                    .map_err(|_| AgentConversationError::StaleGeneration)?,
                None if persisted.work.step.fences_conversation_output() => {
                    return Err(AgentConversationError::StaleGeneration);
                }
                None => persisted.clone(),
            };
            if current.conversation.runtime_generation != Some(runtime_generation) {
                return Err(AgentConversationError::StaleGeneration);
            }
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
            let identity_terminal = is_identity_terminal_for_current(&current, update);
            let (replayed, buffered_replay) = if let Some(source) = &source {
                let (replayed, buffered) = coalesce::replay_state(
                    source,
                    &current,
                    previous.as_ref(),
                    assembly,
                    guard.is_some(),
                )?;
                // An older observer may have durably recorded this exact
                // terminal as a quiet receipt. Preserve the validated source
                // position, but let the replacement observer apply its
                // identity-bearing terminal meaning to the still-active turn.
                (replayed && !identity_terminal, buffered)
            } else {
                (false, false)
            };
            if buffered_replay && !input.flush() && !identity_terminal {
                return Ok(AgentOutputCommit {
                    outcome: AgentUpdateOutcome::quiet(),
                    committed: None,
                });
            }
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
                    // Semantic history can advance after the atomic snapshot.
                    // Retry recovery if that history belongs to a newer aggregate.
                    let refreshed = AgentStore::get_agent(self.store.as_ref(), id)
                        .await
                        .map_err(store_error)?;
                    if refreshed.as_ref() != Some(&persisted) {
                        continue;
                    }
                    return Err(error);
                }
            };
            if assembly.buffered_output().is_some() {
                // Ephemeral thought identity must survive snapshots while its receipt is buffered.
                recovered.live_thought = assembly.live_thought.clone();
            }
            if replayed {
                AgentOutputCategory::Replay.observe();
                // A reconnect may parse an old terminal as a no-op after the turn closed.
                // The immutable source position is the receipt, independent of that parsing.
                if let Some(reanchored) = previous
                    .as_ref()
                    .and_then(|checkpoint| checkpoint.reanchored(&current))
                {
                    match self
                        .commit_output(
                            persisted.clone(),
                            previous.clone(),
                            reanchored,
                            None,
                            guard,
                            false,
                        )
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
                        false,
                    )
                    .await
                    .map_err(store_error)?;
                }
                *assembly =
                    coalesce::accepted(recovered, assembly, source, self.coalesce_policy.is_some());
                return Ok(AgentOutputCommit {
                    outcome: AgentUpdateOutcome::quiet(),
                    committed: source.cloned(),
                });
            }
            if let Some(recorded) = recorded {
                AgentOutputCategory::Replay.observe();
                if let Some(reanchored) = recorded.reanchored(&current) {
                    // An exact receipt retry must also persist a verified repair.
                    // Otherwise a later control write still sees the obsolete anchor.
                    match self
                        .commit_output(
                            persisted.clone(),
                            previous.clone(),
                            reanchored,
                            None,
                            guard,
                            false,
                        )
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
                        false,
                    )
                    .await
                    .map_err(store_error)?;
                }
                *assembly =
                    coalesce::accepted(recovered, assembly, source, self.coalesce_policy.is_some());
                return Ok(AgentOutputCommit {
                    outcome: recorded.outcome.clone(),
                    committed: source.cloned(),
                });
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
            if !current.status.is_live() {
                return Err(AgentConversationError::InvalidState(
                    "conversation updates require a live agent",
                ));
            }
            let update = if guard.is_some_and(|guard| guard.owner_restart.is_some()) {
                output::owner_restart_effect(update)?
            } else {
                update
            };
            let folded = match &update {
                Some(update) => fold_with_output_policy(
                    &current,
                    &recovered,
                    budget::terminal(
                        self.spend_source.as_deref(),
                        &current,
                        if shared {
                            std::borrow::Cow::Borrowed(*update)
                        } else {
                            std::borrow::Cow::Owned((*update).clone())
                        },
                    )
                    .await?,
                    &appended_at,
                    self.writer_policy.allows_writer(
                        &current.owner,
                        crate::sandbox::task_experience::TaskExperienceWriter::ToolOutput,
                    ),
                    self.incremental_text && guard.is_none(),
                )?,
                None => FoldedConversationUpdate::Quiet(recovered.clone()),
            };
            let (next_agent, mut next_assembly, logs, turn_closed) = match folded {
                FoldedConversationUpdate::Quiet(next) => (current.clone(), next, Vec::new(), false),
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
            AgentOutputCategory::observe_fold(!logs.is_empty(), durable_assembly_changed);
            let recovered_anchor = !recovered.is_empty()
                && previous
                    .as_ref()
                    .is_some_and(|checkpoint| !checkpoint.matches(&current));
            if let (Some(source), Some(policy)) = (source, self.coalesce_policy) {
                if input.coalesce()
                    && guard.is_none()
                    && assembly.pending_update.is_none()
                    && logs.is_empty()
                    && !durable_assembly_changed
                    && !recovered_anchor
                    && !identity_terminal
                    && next_assembly.retain_quiet(assembly, source, runtime_generation, policy)
                {
                    *assembly = next_assembly;
                    self.live.publish(fragment);
                    return Ok(AgentOutputCommit {
                        outcome: AgentUpdateOutcome::quiet(),
                        committed: None,
                    });
                }
            }
            if source.is_none() && logs.is_empty() && !durable_assembly_changed && !recovered_anchor
            {
                *assembly = next_assembly;
                self.live.publish(fragment);
                return Ok(AgentOutputCommit {
                    outcome: AgentUpdateOutcome::quiet(),
                    committed: source.cloned(),
                });
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
                assembly: next_assembly.durable_copy(),
                outcome,
                output: output_source_for_checkpoint(
                    source,
                    previous.as_ref(),
                    runtime_generation,
                    identity_terminal,
                ),
            };
            checkpoint.validate().map_err(store_error)?;
            let mutation = (!logs.is_empty()).then(|| AgentConversationMutation {
                settings_fence: None,
                command_claim: None,
                execution_fence: None,
                trigger_execution_fence: None,
                admission_guard: None,
                expected_agent_revision: current.revision,
                expected_last_seq: current.conversation.last_seq,
                runtime_generation,
                agent: next_agent,
                appended_at: appended_at.clone(),
                agent_logs: logs,
            });
            match self
                .commit_output(
                    persisted.clone(),
                    previous,
                    checkpoint,
                    mutation,
                    guard,
                    source.is_some() && identity_terminal,
                )
                .await
            {
                Ok(outcome) => {
                    if outcome.turn_closed {
                        *assembly = coalesce::accepted(
                            AgentMessageAssembly::default(),
                            assembly,
                            source,
                            self.coalesce_policy.is_some(),
                        );
                    } else {
                        *assembly = coalesce::accepted(
                            next_assembly,
                            assembly,
                            source,
                            self.coalesce_policy.is_some(),
                        );
                        self.live.publish(fragment);
                    }
                    return Ok(AgentOutputCommit {
                        outcome,
                        committed: source.cloned(),
                    });
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
}
