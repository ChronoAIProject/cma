use super::*;
use crate::store::{
    agents::read_agent, sandbox_events::event_document, sandboxes::sandbox_replacement_fields,
};
use layer2_core::sandbox::{
    AgentStatus, Sandbox, SandboxEvent, SandboxEventKind, SandboxLifecycleOperation,
    SandboxRevivePhase, SandboxStatus, SandboxWorkStep,
};
use mongodb::Database;

pub(super) async fn apply(
    database: &Database,
    mut snapshot: ChannelLifecycleSnapshot,
    document: &Document,
    permit: &ChannelRevivalAuthority,
    session: &mut ClientSession,
) -> Result<ChannelLifecycleSnapshot, MutationError> {
    if !matches!(
        snapshot.sandbox.lifecycle.operation,
        Some(SandboxLifecycleOperation::Revive {
            phase: SandboxRevivePhase::PublishActive,
            ..
        })
    ) || snapshot
        .sandbox
        .lifecycle
        .home_recovery
        .as_ref()
        .is_some_and(|r| r.resume_pause.is_some())
    {
        return Err(AllocationError::InvalidTransition.into());
    }
    snapshot
        .current_attempt()
        .ok_or(AllocationError::Corrupt)?
        .require_completed_key(&snapshot.allocation, &snapshot.sandbox)?;
    // Fence every real participant against late conversation/queue changes.
    let agents: Collection<Document> = majority_collection(database, "agents");
    let mut cursor = agents
        .find(doc! {"sandbox_id":snapshot.sandbox.id.as_str()})
        .limit(257)
        .session(&mut *session)
        .await?;
    let mut count = 0;
    while let Some(row) = cursor.next(&mut *session).await.transpose()? {
        count += 1;
        if count > 256 {
            return Err(AllocationError::Corrupt.into());
        }
        let agent = read_agent(&row).map_err(|_| AllocationError::Corrupt)?;
        if agent.sandbox_id != snapshot.sandbox.id || agent.owner != snapshot.sandbox.owner {
            return Err(AllocationError::IdentityMismatch.into());
        }
        if snapshot.sandbox.restores_agent(&agent)
            && (agent.status != AgentStatus::Ready
                || agent.conversation.active_turn.is_some()
                || agent
                    .conversation
                    .operation
                    .as_ref()
                    .is_some_and(|operation| operation.blocks_dispatch())
                || agent.work.step.is_pending()
                || agent.work.lease.is_some()
                || agent.requested_stop_reason.is_some()
                || !agent.conversation.queued_messages.is_empty())
        {
            return Err(AllocationError::LeaseUnavailable.into());
        }
        let changed = agents
            .update_one(
                doc! {"_id":agent.id.as_str(),"revision":agent.revision as i64},
                doc! {"$inc":{"channel_lifecycle_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
    }
    let original = snapshot.sandbox.clone();
    let lease = original
        .work
        .lease
        .as_ref()
        .ok_or(AllocationError::LeaseUnavailable)?;
    let stamp = layer2_core::rfc3339_at(SystemTime::now());
    if stamp < original.updated_at {
        return Err(AllocationError::Conflict.into());
    }
    snapshot
        .sandbox
        .publish_lifecycle_active(stamp.clone(), &lease.holder, lease.fence, stamp.clone())
        .map_err(|_| AllocationError::InvalidTransition)?;
    // Dedicated human-authorized publication selects the Channel scheduler.
    // No ordinary lifecycle mutation or scheduling permission is widened.
    snapshot.sandbox.work.step = SandboxWorkStep::None;
    snapshot
        .sandbox
        .validate()
        .map_err(|_| AllocationError::Corrupt)?;
    super::audit::append(
        database,
        &snapshot,
        permit,
        &LifecycleCredentialChange::Publish,
        session,
    )
    .await?;
    let mut fields =
        sandbox_replacement_fields(&snapshot.sandbox).map_err(|_| AllocationError::Corrupt)?;
    events(
        database,
        document,
        &snapshot.sandbox,
        &mut fields,
        vec![
            SandboxEventKind::StatusChanged {
                from: original.status,
                to: SandboxStatus::Active,
            },
            SandboxEventKind::Revived,
        ],
        session,
    )
    .await?;
    let rows: Collection<Document> = majority_collection(database, "sandboxes");
    let changed = rows.update_one(doc! {"_id":original.id.as_str(),"revision":original.revision as i64}, doc! {"$set":fields,"$unset":{"channel_renewal":"","channel_renewal_retry_at_ms":"","channel_renewal_retry_state":""}}).session(&mut *session).await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    snapshot.renewal = None;
    Ok(snapshot)
}

pub(super) async fn events(
    database: &Database,
    document: &Document,
    sandbox: &Sandbox,
    fields: &mut Document,
    kinds: Vec<SandboxEventKind>,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let mut seq = document
        .get_i64("last_seq")
        .map_err(|_| AllocationError::Corrupt)?;
    let rows: Collection<Document> = majority_collection(database, "sandbox_events");
    for kind in kinds {
        seq = seq.checked_add(1).ok_or(AllocationError::Corrupt)?;
        rows.insert_one(
            event_document(&SandboxEvent {
                sandbox_id: sandbox.id.clone(),
                seq: seq as u64,
                appended_at: sandbox.updated_at.clone(),
                kind,
            })
            .map_err(|_| AllocationError::Corrupt)?,
        )
        .session(&mut *session)
        .await?;
    }
    fields.insert("last_seq", seq);
    Ok(())
}
