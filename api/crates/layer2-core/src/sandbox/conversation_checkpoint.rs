//! Recover an existing private prefix across append-only connection publications.

use super::{store_error, DurableAgentUpdateRecorder, MAX_CONVERSATION_COMMIT_RETRIES};
use crate::sandbox::store::{
    AgentConversationUpdateStore, AgentMessageCheckpoint, AgentStore, SandboxStoreError,
    MAX_PAGE_SIZE,
};
use crate::sandbox::{
    Agent, AgentConversationError, AgentLogKind, AgentMessageAssembly, AgentWorkStep,
};

pub(super) async fn recover_assembly(
    store: &dyn AgentConversationUpdateStore,
    current: &Agent,
    checkpoint: Option<&AgentMessageCheckpoint>,
) -> Result<AgentMessageAssembly, AgentConversationError> {
    let Some(checkpoint) = checkpoint else {
        return Ok(AgentMessageAssembly::default());
    };
    if checkpoint.matches(current) {
        return Ok(checkpoint.assembly.clone());
    }
    if checkpoint.assembly.is_empty()
        || current.conversation.conversation_id.as_deref()
            != Some(&checkpoint.native_conversation_id)
        || current.conversation.runtime_generation != Some(checkpoint.runtime_generation)
        || current.conversation.active_turn != checkpoint.active_turn
    {
        return Ok(AgentMessageAssembly::default());
    }
    let stale = || {
        AgentConversationError::Corrupt("active message checkpoint has a stale log anchor".into())
    };
    // Older writers can append reconnection evidence without advancing the sidecar.
    // Validate only a bounded suffix; normal chunks still perform no history read.
    // The next checkpoint commit CASes both the captured Agent and original sidecar,
    // so another publication during this read forces an ordinary recorder retry.
    let count = current
        .conversation
        .last_seq
        .checked_sub(checkpoint.after_seq)
        .filter(|count| *count > 0 && *count <= u64::from(MAX_PAGE_SIZE))
        .ok_or_else(stale)?;
    let suffix = store
        .agent_log(&current.id, checkpoint.after_seq, count as u16)
        .await
        .map_err(store_error)?;
    if suffix.len() != count as usize
        || suffix.iter().enumerate().any(|(index, entry)| {
            entry.agent_id != current.id
                || entry.seq != checkpoint.after_seq + index as u64 + 1
                || !matches!(
                    entry.kind,
                    AgentLogKind::WorkStepCompleted {
                        step: AgentWorkStep::PublishConnection,
                    } | AgentLogKind::WorkStepFailed {
                        step: AgentWorkStep::PublishConnection,
                        ..
                    }
                )
        })
    {
        return Err(stale());
    }
    Ok(checkpoint.assembly.clone())
}

pub(super) async fn restore(
    recorder: &DurableAgentUpdateRecorder,
    id: &crate::AgentId,
    runtime_generation: u64,
    assembly: &mut AgentMessageAssembly,
) -> Result<(), AgentConversationError> {
    if assembly.pending_update.is_some() || assembly.buffered_output().is_some() {
        return Err(AgentConversationError::InvalidState(
            "an uncertain update must be retried before restoring its assembly",
        ));
    }
    for _ in 0..MAX_CONVERSATION_COMMIT_RETRIES {
        let snapshot = recorder
            .store
            .agent_message_snapshot(id)
            .await
            .map_err(store_error)?
            .ok_or(AgentConversationError::NotFound)?;
        let current = snapshot.agent;
        let previous = snapshot.checkpoint;
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
        let recovered =
            match recover_assembly(recorder.store.as_ref(), &current, previous.as_ref()).await {
                Ok(recovered) => recovered,
                Err(error) => {
                    if AgentStore::get_agent(recorder.store.as_ref(), id)
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
            match recorder
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
