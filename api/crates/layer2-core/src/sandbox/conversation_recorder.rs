use std::sync::Arc;

use layer3_adapter::ConversationUpdate;
use sha2::{Digest, Sha256};

use crate::ids::AgentId;
use crate::seconds_from_rfc3339;

use super::conversation::PendingAgentUpdate;
use super::conversation_fold::{fold_update, FoldedConversationUpdate};
use super::store::{
    AgentConversationMutation, AgentConversationUpdateStore, AgentMessageCheckpoint, AgentStore,
    SandboxStoreError,
};
use super::{
    AgentConversationError, AgentMessageAssembly, AgentUpdateOutcome, AgentUpdateRecorder,
};

const MAX_CONVERSATION_COMMIT_RETRIES: usize = 8;

#[path = "conversation_checkpoint.rs"]
mod checkpoint;

#[path = "conversation_output.rs"]
mod output;
use output::output_replayed;

#[path = "conversation_live.rs"]
pub(super) mod live;

pub use live::{
    AgentLiveFragment, AgentLiveFragmentKind, AgentLivePublisher, AgentLiveSource,
    AgentLiveSubscription,
};

pub struct DurableAgentUpdateRecorder {
    store: Arc<dyn AgentConversationUpdateStore>,
    live: live::AgentLiveHub,
    live_source: std::sync::OnceLock<Arc<dyn AgentLiveSource>>,
    writer_policy: Arc<super::task_experience::TaskExperiencePolicy>,
}

mod configuration;

#[async_trait::async_trait]
impl AgentUpdateRecorder for DurableAgentUpdateRecorder {
    fn subscribe_live(&self, id: &AgentId) -> Option<AgentLiveSubscription> {
        match self.live_source.get() {
            Some(source) => source.subscribe(id),
            None => Some(self.subscribe_local(id)),
        }
    }

    async fn restore_assembly(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
    ) -> Result<(), AgentConversationError> {
        if assembly.pending_update.is_some() {
            return Err(AgentConversationError::InvalidState(
                "an uncertain update must be retried before restoring its assembly",
            ));
        }
        for _ in 0..MAX_CONVERSATION_COMMIT_RETRIES {
            let current = AgentStore::get_agent(self.store.as_ref(), id)
                .await
                .map_err(store_error)?
                .ok_or(AgentConversationError::NotFound)?;
            if runtime_generation == 0
                || current.conversation.runtime_generation != Some(runtime_generation)
            {
                return Err(AgentConversationError::StaleGeneration);
            }
            if !current.status.is_live() {
                return Err(AgentConversationError::InvalidState(
                    "assembly restoration requires a live agent",
                ));
            }
            let previous = self
                .store
                .agent_message_checkpoint(id)
                .await
                .map_err(store_error)?;
            let recovered = match checkpoint::recover_assembly(
                self.store.as_ref(),
                &current,
                previous.as_ref(),
            )
            .await
            {
                Ok(recovered) => recovered,
                Err(error) => {
                    if AgentStore::get_agent(self.store.as_ref(), id)
                        .await
                        .map_err(store_error)?
                        .as_ref()
                        != Some(&current)
                    {
                        continue;
                    }
                    return Err(error);
                }
            };
            if let Some(next) = previous
                .as_ref()
                .and_then(|value| value.reanchored(&current))
            {
                match self
                    .store
                    .commit_agent_message_update(current, previous, next, None)
                    .await
                {
                    Ok(_) => {}
                    Err(SandboxStoreError::Conflict) => continue,
                    Err(error) => return Err(store_error(error)),
                }
            }
            *assembly = recovered;
            return Ok(());
        }
        Err(AgentConversationError::Unavailable(
            "conversation changed during every restoration attempt".into(),
        ))
    }

    async fn record_reopen_update(
        &self,
        id: &AgentId,
        guard: &super::store::AgentReopenGuard,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        self.record_with_reopen(
            id,
            guard.runtime_generation,
            assembly,
            update,
            appended_at,
            Some(guard),
        )
        .await
    }

    async fn record_update(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
    ) -> Result<AgentUpdateOutcome, AgentConversationError> {
        self.record_with_reopen(id, runtime_generation, assembly, update, appended_at, None)
            .await
    }
}

impl DurableAgentUpdateRecorder {
    async fn record_with_reopen(
        &self,
        id: &AgentId,
        runtime_generation: u64,
        assembly: &mut AgentMessageAssembly,
        update: ConversationUpdate,
        appended_at: String,
        guard: Option<&super::store::AgentReopenGuard>,
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
            let replayed = if let Some(source) = &source {
                output_replayed(source, &current, previous.as_ref())?
            } else {
                false
            };
            if let Some(recorded) = recorded {
                if recorded.update_fingerprint != fingerprint {
                    return Err(AgentConversationError::Corrupt(
                        "checkpoint update identity conflict".into(),
                    ));
                }
            }
            let recovered = match checkpoint::recover_assembly(
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
            if replayed {
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
                        super::task_experience::TaskExperienceWriter::ToolOutput,
                    ),
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
                    .map(|(index, kind)| super::AgentLogEntry {
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
}

fn store_error(error: SandboxStoreError) -> AgentConversationError {
    match error {
        SandboxStoreError::NotFound => AgentConversationError::NotFound,
        SandboxStoreError::InvalidTransaction(detail) => {
            AgentConversationError::InvalidState(detail)
        }
        SandboxStoreError::AgentLogCapacityExceeded => AgentConversationError::CapacityExceeded,
        SandboxStoreError::Corrupt(detail) => AgentConversationError::Corrupt(detail),
        other => AgentConversationError::Unavailable(other.to_string()),
    }
}
