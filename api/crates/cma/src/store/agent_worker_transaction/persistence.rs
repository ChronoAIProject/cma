//! Aggregate CAS and worktree writes inside the publication transaction.
use super::*;

pub(super) fn is_effect_free_workspace_wait_requeue(mutation: &AgentWorkMutation) -> bool {
    // An ordinary wait is immediately due. A requeue after a confirmed abort may be delayed.
    mutation.agent.work.step == AgentWorkStep::WaitForWorkspace
        && mutation.agent.work.ready_at >= mutation.appended_at
        && mutation.agent.work.lease.is_none()
        && mutation.agent_logs.is_empty()
        && mutation.worktree.is_none()
        && mutation.sandbox_event.is_none()
}

pub(super) fn fence_workspace_wait_lease(
    filter: &mut Document,
    current: &Agent,
    mutation: &AgentWorkMutation,
) {
    if current.work.step == AgentWorkStep::WaitForWorkspace
        && (mutation.agent.work.step == AgentWorkStep::AddWorktree
            || is_effect_free_workspace_wait_requeue(mutation))
    {
        filter.insert(
            "$expr",
            doc! {
                "$gt": [
                    { "$dateFromString": {
                        "dateString": "$work_lease_expires_at", "onError": null, "onNull": null,
                    }},
                    "$$NOW",
                ],
            },
        );
    }
}

pub(super) async fn advance_agent(
    store: &MongoSandboxStore,
    current_row: &Document,
    current: &Agent,
    next: &Agent,
    mutation: &AgentWorkMutation,
    integrity: mongodb::bson::Document,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let lease = current
        .work
        .lease
        .as_ref()
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    let mut fields = agent_replacement_fields(next).map_err(ApplyError::Domain)?;
    crate::store::agent_conversation_store::carry_checkpoint_fields(
        current_row,
        current,
        next,
        &mut fields,
    )
    .map_err(ApplyError::Domain)?;
    fields.insert("agent_log_integrity_v1", integrity);
    fields.insert(
        "last_seq",
        signed(next.conversation.last_seq, "agent log sequence")
            .map_err(SandboxStoreError::Corrupt)
            .map_err(ApplyError::Domain)?,
    );
    let mut filter = doc! {
        "_id": current.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": signed(mutation.expected_agent_revision, "agent revision")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        "last_seq": signed(current.conversation.last_seq, "agent log sequence")
            .map_err(SandboxStoreError::Corrupt)
            .map_err(ApplyError::Domain)?,
        "channel_reuse_cleanup_execution": { "$exists": false },
        "channel_history_seal": { "$exists": false },
        "work_lease_holder": &mutation.holder,
        "work_lease_fence": signed(mutation.fence, "agent work lease fence")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        "work_lease_expires_at": &lease.expires_at,
        "work_next_fence": signed(current.work.next_fence, "agent work next fence")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
    };
    owner_stop::fence_live_lease(&mut filter, mutation);
    restart_ready::fence_live_lease(&mut filter, current);
    fence_workspace_wait_lease(&mut filter, current, mutation);
    let advanced = store
        .agents
        .update_one(filter, doc! { "$set": fields })
        .session(session)
        .await
        .map_err(classify_aggregate_write)?;
    if advanced.matched_count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}

pub(super) async fn advance_worktree(
    store: &MongoSandboxStore,
    current: &Worktree,
    next: &Worktree,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let agent_id = current
        .agent_id
        .as_ref()
        .map(|id| Bson::String(id.as_str().to_owned()))
        .unwrap_or(Bson::Null);
    let advanced = store
        .worktrees
        .update_one(
            doc! {
                "_id": current.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "workspace_id": current.workspace_id.as_str(),
                "sandbox_id": current.sandbox_id.as_str(),
                "owner": current.owner.as_str(),
                "name": &current.name,
                "status": record_bson(&current.status)
                    .map_err(SandboxStoreError::Corrupt)
                    .map_err(ApplyError::Domain)?,
                "revision": signed(current.revision, "worktree revision")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                "record.path": &current.path,
                "record.branch": &current.branch,
                "record.base_commit": &current.base_commit,
                "record.agent_id": agent_id,
            },
            doc! { "$set": worktree_replacement_fields(next).map_err(ApplyError::Domain)? },
        )
        .session(session)
        .await
        .map_err(classify_aggregate_write)?;
    if advanced.matched_count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}

fn classify_aggregate_write(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::Duplicate)
    } else {
        ApplyError::Driver(error)
    }
}
