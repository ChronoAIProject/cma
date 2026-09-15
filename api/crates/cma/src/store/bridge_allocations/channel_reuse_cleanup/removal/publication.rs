use super::*;
use crate::store::{
    agent_log::{log_document, scan_complete_log_with_session},
    agent_log_integrity::{ensure_append_capacity, AppendCapacity},
    agents::agent_replacement_fields,
    sandbox_mongo::MongoSandboxStore,
    worktrees::{read_worktree, worktree_replacement_fields},
};
use layer2_core::sandbox::{
    AgentLogEntry, AgentLogKind, AgentStatus, AgentStopReason, WorktreeStatus,
};

pub(super) async fn finish(
    database: &Database,
    resources: &MongoSandboxStore,
    state: &ChannelReuseRemovalState,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let target = &state.seal.target;
    let agent_row = read(database, "agents", target.agent_id.as_str(), session).await?;
    let tree_row = read(database, "worktrees", target.worktree_id.as_str(), session).await?;
    let mut agent = read_agent(&agent_row).map_err(|_| AllocationError::Corrupt)?;
    let mut tree = read_worktree(&tree_row).map_err(|_| AllocationError::Corrupt)?;
    let now = layer2_core::now_rfc3339();
    let reason = AgentStopReason::Owner;
    agent
        .transition(AgentStatus::Stopped, now.clone(), Some(reason.clone()))
        .map_err(|_| AllocationError::Corrupt)?;
    agent
        .transition(AgentStatus::Deleted, now.clone(), None)
        .map_err(|_| AllocationError::Corrupt)?;
    agent.work.next_fence = state
        .authority_fence
        .checked_add(1)
        .ok_or(AllocationError::Corrupt)?;
    tree.transition(WorktreeStatus::Removing)
        .map_err(|_| AllocationError::Corrupt)?;
    tree.transition(WorktreeStatus::Removed)
        .map_err(|_| AllocationError::Corrupt)?;
    let previous_seq = agent.conversation.last_seq;
    let logs = [
        AgentLogKind::Stopped {
            reason: reason.as_str().into(),
        },
        AgentLogKind::Deleted,
    ]
    .into_iter()
    .enumerate()
    .map(|(offset, kind)| {
        Ok(AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: previous_seq
                .checked_add(offset as u64 + 1)
                .ok_or(AllocationError::Corrupt)?,
            appended_at: now.clone(),
            kind,
        })
    })
    .collect::<Result<Vec<_>, AllocationError>>()?;
    let summary =
        scan_complete_log_with_session(resources, &agent.id, previous_seq, session, |_| Ok(()))
            .await
            .map_err(|_| AllocationError::Corrupt)?;
    ensure_append_capacity(&summary, &logs, AppendCapacity::Delete)
        .map_err(|_| AllocationError::Corrupt)?;
    agent.conversation.last_seq = previous_seq
        .checked_add(2)
        .ok_or(AllocationError::Corrupt)?;
    agent
        .validate_conversation()
        .map_err(|_| AllocationError::Corrupt)?;
    let mut fields = agent_replacement_fields(&agent).map_err(|_| AllocationError::Corrupt)?;
    fields.insert(
        "last_seq",
        i64::try_from(agent.conversation.last_seq).map_err(|_| AllocationError::Corrupt)?,
    );
    for (name, row, fields) in [
        ("agents", agent_row, fields),
        (
            "worktrees",
            tree_row,
            worktree_replacement_fields(&tree).map_err(|_| AllocationError::Corrupt)?,
        ),
    ] {
        let changed = majority_collection::<Document>(database, name)
            .update_one(row, doc! {"$set":fields})
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
    }
    for log in logs {
        resources
            .agent_log
            .insert_one(log_document(&log).map_err(|_| AllocationError::Corrupt)?)
            .session(&mut *session)
            .await?;
    }
    // The permanent child marker and tombstone survive. Only the temporary
    // parent/Workspace exclusion is released, after all physical receipts.
    for (name, id) in [
        ("sandboxes", target.sandbox_id.as_str()),
        ("workspaces", target.workspace_id.as_str()),
    ] {
        let changed = majority_collection::<Document>(database, name)
            .update_one(
                doc! {"_id":id,CLEANUP_EXECUTION_FIELD:&state.id},
                doc! {"$unset":{CLEANUP_EXECUTION_FIELD:""}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
    }
    Ok(())
}

pub(in crate::store) async fn require_complete(
    database: &Database,
    state: &ChannelReuseRemovalState,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    state.validate()?;
    if state.phase != ChannelReuseRemovalPhase::Complete {
        return Err(AllocationError::Corrupt.into());
    }
    let agent_row = read(
        database,
        "agents",
        state.seal.target.agent_id.as_str(),
        session,
    )
    .await?;
    let tree_row = read(
        database,
        "worktrees",
        state.seal.target.worktree_id.as_str(),
        session,
    )
    .await?;
    let agent = read_agent(&agent_row).map_err(|_| AllocationError::Corrupt)?;
    let tree = read_worktree(&tree_row).map_err(|_| AllocationError::Corrupt)?;
    let target = &state.seal.target;
    let owner = match &state.seal.owner {
        layer2_core::resource_access::ResourceOwner::Person { subject }
        | layer2_core::resource_access::ResourceOwner::Organization { subject } => subject,
    };
    // The caller retains the full ResourceOwner comparison. Child aggregates
    // store only its subject, and must still prove every sealed lineage edge.
    if agent.id != target.agent_id
        || &agent.owner != owner
        || agent.sandbox_id != target.sandbox_id
        || agent.workspace_id != target.workspace_id
        || agent.status != AgentStatus::Deleted
        || agent.deleted_at.is_none()
        || agent.created_at != state.seal.agent_created_at
        || agent.worktree_id != target.worktree_id
        || tree.id != target.worktree_id
        || &tree.owner != owner
        || tree.sandbox_id != target.sandbox_id
        || tree.workspace_id != target.workspace_id
        || tree.agent_id.as_ref() != Some(&target.agent_id)
        || tree.status != WorktreeStatus::Removed
        || tree.path != state.seal.worktree_path
        || agent_row.get_str(CLEANUP_EXECUTION_FIELD).ok() != Some(state.id.as_str())
        || tree_row.get_str(CLEANUP_EXECUTION_FIELD).ok() != Some(state.id.as_str())
    {
        return Err(AllocationError::Corrupt.into());
    }
    // Serialize the exact terminal proof with restore/capacity publication.
    // A concurrent child mutation cannot invalidate a successful observation.
    fence_row(database, "agents", agent_row, session).await?;
    fence_row(database, "worktrees", tree_row, session).await?;
    Ok(())
}
