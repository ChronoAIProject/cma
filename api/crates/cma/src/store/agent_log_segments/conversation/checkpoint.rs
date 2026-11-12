//! Scoped checkpoint domain validation; the root alone commits or aborts.
use super::*;
use domain::{
    capacity_prefix,
    checkpoint::{read, replay, replay_at_anchor, write, Authority},
};
use layer2_core::sandbox::{store::AgentMessageCheckpoint, AgentUpdateOutcome};

pub(in crate::store) async fn apply(
    store: &MongoSandboxStore,
    source: (&Agent, Option<&adoption::Prepared>),
    previous: Option<&AgentMessageCheckpoint>,
    next: &AgentMessageCheckpoint,
    mutation: Option<&AgentConversationMutation>,
    authority: &Authority,
    scope: &mut TransactionScope<'_>,
) -> Result<(AgentUpdateOutcome, Option<ConversationReceipt>), ApplyError> {
    let (current, adoption) = source;
    let guard = match authority {
        Authority::Reopen(guard) => Some(guard.as_ref()),
        _ => None,
    };
    let capacity = match authority {
        Authority::Capacity(proof) => Some(proof.as_ref()),
        _ => None,
    };
    let row = store
        .agents
        .find_one(doc! { "_id": current.id.as_str() })
        .session(scope.session())
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let actual_agent = read_agent(&row).map_err(ApplyError::Domain)?;
    let actual = read(&row).map_err(ApplyError::Domain)?;
    if let Some(guard) = guard {
        if actual_agent != *current {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        domain::reopen::validate_and_fence(store, &actual_agent, guard, scope.session()).await?;
    } else if actual.as_ref().is_some_and(|checkpoint| {
        Some(checkpoint.runtime_generation) > current.conversation.runtime_generation
    }) {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let project = |agent: &Agent| match guard {
        Some(guard) => guard.project(agent).map_err(ApplyError::Domain),
        None => Ok(agent.clone()),
    };
    if mutation.is_none()
        && previous
            .and_then(|value| value.reanchored(current))
            .as_ref()
            == Some(next)
    {
        if let Some(outcome) =
            replay_at_anchor(actual.as_ref(), next, true).map_err(ApplyError::Domain)?
        {
            super::require_replay(store, mutation, next, capacity.is_some(), scope).await?;
            return Ok((outcome, None));
        }
        if actual_agent != *current || actual.as_ref() != previous || !current.status.is_live() {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        write(store, current, Some(next), scope.session()).await?;
        return Ok((next.outcome.clone(), None));
    }
    if let Some(outcome) = replay(actual.as_ref(), next).map_err(ApplyError::Domain)? {
        super::require_replay(store, mutation, next, capacity.is_some(), scope).await?;
        return Ok((outcome, None));
    }
    if actual_agent != *current || actual.as_ref() != previous || !current.status.is_live() {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let mut publication = None;
    let updated = if let Some(mutation) = mutation {
        if mutation.agent.id != current.id || !next.matches(&project(&mutation.agent)?) {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        if let Some(proof) = capacity {
            proof
                .validate_snapshot(&actual_agent, actual.as_ref())
                .map_err(ApplyError::Domain)?;
        }
        let receipt = super::apply(
            ScanCaller::Checkpoint,
            store,
            mutation,
            capacity,
            Some(next),
            adoption,
            scope,
        )
        .await?;
        if receipt.logs != next.outcome.appended {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        publication = Some(receipt);
        &mutation.agent
    } else {
        if !next.matches(&project(current)?)
            || !next.outcome.appended.is_empty()
            || next.outcome.turn_closed
        {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        current
    };
    write(store, updated, Some(next), scope.session()).await?;
    if let Some(proof) = capacity {
        // Replace the active checkpoint with the compact receipt before adding
        // the retained prefix, avoiding two large checkpoints in one document.
        // Both writes remain part of this same transaction.
        capacity_prefix::preserve(store, &row, proof, scope.session()).await?;
    }
    Ok((next.outcome.clone(), publication))
}
