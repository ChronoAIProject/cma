//! Scoped checkpoint domain validation; the root alone commits or aborts.
use super::*;
use domain::{
    capacity_prefix,
    checkpoint::{read, replay, replay_at_anchor, write, write_from_row, Authority},
};
use layer2_core::sandbox::{store::AgentMessageCheckpoint, AgentUpdateOutcome};

pub(in crate::store) async fn apply_turn_policy(
    store: &MongoSandboxStore,
    proof: &layer2_core::sandbox::turn_timeout::TurnTimeoutSettlement,
    scope: &mut TransactionScope<'_>,
) -> Result<layer2_core::sandbox::agent_io::AdmittedAgentCommand, ApplyError> {
    let conflict = || ApplyError::Domain(SandboxStoreError::Conflict);
    let snapshot = proof.snapshot();
    let lane = scope
        .find_one(
            &store.agent_command_orders,
            doc! {"_id":snapshot.agent.id.as_str()},
        )
        .await
        .map_err(ApplyError::Driver)?
        .ok_or_else(conflict)?;
    let mut lane =
        crate::store::agent_commands::lane_records::read(&lane, snapshot.agent.id.as_str())
            .map_err(|_| conflict())?;
    let settled = proof.settle_command(&mut lane).map_err(|_| conflict())?;
    let (_, publication) = Box::pin(apply(
        store,
        (&snapshot.agent, None),
        snapshot.checkpoint.as_ref(),
        proof.checkpoint(),
        Some(proof.mutation()),
        &Authority::TurnPolicy,
        scope,
    ))
    .await?;
    if publication.is_none() {
        return Err(conflict());
    }
    crate::store::agent_commands::worker_apply::write(
        store,
        &settled,
        proof.command().revision,
        &lane,
        scope.session(),
    )
    .await
    .map_err(|error| match error {
        crate::store::agent_commands::ApplyError::Driver(error) => ApplyError::Driver(error),
        _ => conflict(),
    })?;
    Ok(settled)
}

pub(in crate::store) async fn apply_stale_turn(
    store: &MongoSandboxStore,
    proof: &layer2_core::sandbox::store::StaleTurnSettlement,
    scope: &mut TransactionScope<'_>,
) -> Result<(), ApplyError> {
    let snapshot = proof.snapshot();
    // The ordinary publication hook writes the current Mongo membership
    // and owner fences before appending. The root requires that they exist.
    let (_, published) = Box::pin(apply(
        store,
        (&snapshot.agent, None),
        snapshot.checkpoint.as_ref(),
        proof.checkpoint(),
        Some(proof.mutation()),
        &Authority::StaleTurn,
        scope,
    ))
    .await?;
    if published.is_none() {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(())
}

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
    let row = scope
        .find_one(&store.agents, doc! { "_id": current.id.as_str() })
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let actual_agent = read_agent(&row).map_err(ApplyError::Domain)?;
    let actual = read(&row).map_err(ApplyError::Domain)?;
    if let Some(guard) = guard {
        if actual_agent != *current {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        Box::pin(domain::reopen::validate_and_fence(
            store,
            &actual_agent,
            guard,
            scope.session(),
        ))
        .await?;
    } else if actual_agent.work.step.fences_conversation_output()
        || actual.as_ref().is_some_and(|checkpoint| {
            Some(checkpoint.runtime_generation) > current.conversation.runtime_generation
        })
    {
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
            Box::pin(super::require_replay(
                store,
                mutation,
                next,
                capacity.is_some(),
                scope,
            ))
            .await?;
            return Ok((outcome, None));
        }
        if actual_agent != *current || actual.as_ref() != previous || !current.status.is_live() {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        write_from_row(store, current, &row, Some(next), scope.session()).await?;
        return Ok((next.outcome.clone(), None));
    }
    if let Some(outcome) = replay(actual.as_ref(), next).map_err(ApplyError::Domain)? {
        Box::pin(super::require_replay(
            store,
            mutation,
            next,
            capacity.is_some(),
            scope,
        ))
        .await?;
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
        let receipt = Box::pin(super::apply(
            match authority {
                Authority::StaleTurn => PublicationSource::StaleTurn,
                Authority::TurnPolicy => PublicationSource::TurnPolicy,
                _ => PublicationSource::Checkpoint,
            },
            store,
            mutation,
            capacity,
            Some(next),
            adoption,
            scope,
        ))
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
    if matches!(authority, Authority::StaleTurn | Authority::TurnPolicy) {
        // Both writers set the exact checkpoint in the publication CAS.
    } else if mutation.is_some() {
        // Publication can adopt or advance the log envelope. Its original row
        // cannot certify the new format/head; retain the post-publication read.
        write(store, updated, Some(next), scope.session()).await?;
    } else {
        write_from_row(store, updated, &row, Some(next), scope.session()).await?;
    }
    if let Some(proof) = capacity {
        // Replace the active checkpoint with the compact receipt before adding
        // the retained prefix, avoiding two large checkpoints in one document.
        // Both writes remain part of this same transaction.
        Box::pin(capacity_prefix::preserve(
            store,
            &row,
            proof,
            scope.session(),
        ))
        .await?;
    }
    Ok((next.outcome.clone(), publication))
}
