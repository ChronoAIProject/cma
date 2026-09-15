//! Durable exclusion and tombstones precede any exact child effect.
use super::*;
use crate::store::agents::read_agent;

mod publication;
mod runtime;
pub(in crate::store) use publication::require_complete;
const REMOVAL_AUDITS: &str = "channel_reuse_cleanup_removal_audits";

impl MongoBridgeAllocationStore {
    pub(in crate::store::bridge_allocations) async fn cleanup_released_agent_capacity(
        &self,
        allocation: &AllocationRecord,
        session: &mut ClientSession,
    ) -> Result<bool, MutationError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let id = format!(
            "{}:child-removal:v1",
            ChannelReuseCleanupState::id(&allocation.id)
        );
        let row = majority_collection::<Document>(&database, CLEANUP_TOMBSTONES)
            .find_one(doc! {"_id":id})
            .session(&mut *session)
            .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let state = read_removal(&row)?;
        if state.phase != ChannelReuseRemovalPhase::Complete {
            return Ok(false);
        }
        if state.seal.allocation_sha256 != digest(allocation)?
            || state.seal.owner != allocation.owner
            || state.seal.target != *allocation.placement.target()
        {
            return Err(AllocationError::Corrupt.into());
        }
        let audit = read(&database, REMOVAL_AUDITS, &audit_id(&state), session).await?;
        if read_removal(&audit)? != state {
            return Err(AllocationError::Corrupt.into());
        }
        publication::require_complete(&database, &state, session).await?;
        fence_row(&database, CLEANUP_TOMBSTONES, row, session).await?;
        Ok(true)
    }

    async fn removal_transaction(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        request: &ChannelReuseCleanupRequest,
        completed: Option<&ChannelReuseRemovalState>,
        session: &mut ClientSession,
    ) -> Result<ChannelReuseRemovalState, MutationError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let saved = self
            .cleanup_request_transaction(permit, request, session)
            .await?;
        let admission = self
            .require_cleanup_admission(&database, &saved, false, session)
            .await?;
        let id = format!("{}:child-removal:v1", saved.id);
        let collection = majority_collection::<Document>(&database, CLEANUP_TOMBSTONES);
        let existing = collection
            .find_one(doc! {"_id":&id})
            .session(&mut *session)
            .await?;
        let previous = existing.as_ref().map(read_removal).transpose()?;
        if let Some(previous) = &previous {
            if previous.request_sha256 != saved.digest
                || previous.admission_sha256 != admission.digest
                || previous.seal != saved.seal
            {
                return Err(AllocationError::IdentityMismatch.into());
            }
            let audit = read(&database, REMOVAL_AUDITS, &audit_id(previous), session).await?;
            if read_removal(&audit)? != *previous {
                return Err(AllocationError::Corrupt.into());
            }
            if previous.phase == ChannelReuseRemovalPhase::Complete {
                publication::require_complete(&database, previous, session).await?;
                return Ok(previous.clone());
            }
        }
        let observed = self
            .cleanup_drain_transaction(permit, request, false, session)
            .await?;
        // C removes the exact Agent/home/worktree only. Foreign Workspace
        // children prohibit whole-Workspace cleanup, which is not performed.
        if observed
            .holds
            .iter()
            .any(|hold| *hold != ChannelReuseCleanupHold::OtherWorkspaceChildren)
        {
            return Err(AllocationError::Conflict.into());
        }
        let agent_row = read(
            &database,
            "agents",
            saved.seal.target.agent_id.as_str(),
            session,
        )
        .await?;
        let agent = read_agent(&agent_row).map_err(|_| AllocationError::Corrupt)?;
        let state = match &previous {
            Some(previous) => {
                if previous.runtime != observed.runtime
                    || agent.work.next_fence > previous.authority_fence
                {
                    return Err(AllocationError::IdentityMismatch.into());
                }
                previous.clone()
            }
            None => ChannelReuseRemovalState::new(&saved, &observed, agent.work.next_fence)?,
        };
        require_markers(&database, &state, previous.is_none(), session).await?;
        if let Some(completed) = completed {
            if completed != &state {
                return Err(AllocationError::Conflict.into());
            }
            let next = state.advance(mongodb::bson::DateTime::now().timestamp_millis())?;
            if next.phase == ChannelReuseRemovalPhase::Complete {
                publication::finish(
                    &database,
                    &self
                        .invokes
                        .as_ref()
                        .ok_or(AllocationError::Unavailable)?
                        .resources,
                    &next,
                    session,
                )
                .await?;
            }
            let changed = collection
                .update_one(
                    existing.ok_or(AllocationError::Corrupt)?,
                    doc! {"$set":removal_document(&next)?},
                )
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 {
                return Err(AllocationError::Conflict.into());
            }
            append_audit(&database, &next, session).await?;
            permit.require_current()?;
            return Ok(next);
        }
        match existing {
            None => {
                collection
                    .insert_one(removal_document(&state)?)
                    .session(&mut *session)
                    .await?;
                append_audit(&database, &state, session).await?;
            }
            Some(row) => fence_row(&database, CLEANUP_TOMBSTONES, row, session).await?,
        }
        permit.require_current()?;
        Ok(state)
    }

    pub(crate) async fn prepare_channel_reuse_child_removal(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        request: &ChannelReuseCleanupRequest,
    ) -> Result<ChannelReuseRemovalState, AllocationError> {
        self.change_removal(permit, request, None).await
    }

    async fn change_removal(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        request: &ChannelReuseCleanupRequest,
        completed: Option<&ChannelReuseRemovalState>,
    ) -> Result<ChannelReuseRemovalState, AllocationError> {
        request.require(permit.allocation())?;
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1 << (attempt - 1).min(5)))
                    .await;
            }
            permit.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let state = match self
                .removal_transaction(permit, request, completed, &mut session)
                .await
            {
                Ok(state) => state,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Mongo(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue
                        }
                        MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                        MutationError::Core(error) => return Err(error),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(state),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable)
                }
            }
        }
        Err(AllocationError::Unavailable)
    }
}

async fn require_markers(
    database: &Database,
    state: &ChannelReuseRemovalState,
    insert: bool,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let target = &state.seal.target;
    for (name, id) in [
        ("sandboxes", target.sandbox_id.as_str()),
        ("workspaces", target.workspace_id.as_str()),
        ("worktrees", target.worktree_id.as_str()),
        ("agents", target.agent_id.as_str()),
    ] {
        let row = read(database, name, id, session).await?;
        match row.get_str(CLEANUP_EXECUTION_FIELD) {
            Ok(id) if !insert && id == state.id => {}
            Err(_) if insert && !row.contains_key(CLEANUP_EXECUTION_FIELD) => {}
            _ => return Err(AllocationError::Conflict.into()),
        }
        let changed = majority_collection::<Document>(database, name).update_one(row,
            doc! {"$set":{CLEANUP_EXECUTION_FIELD:&state.id},"$inc":{"channel_reuse_cleanup_fence":1_i64}})
            .session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
    }
    Ok(())
}

fn removal_document(state: &ChannelReuseRemovalState) -> Result<Document, AllocationError> {
    state.validate()?;
    Ok(
        doc! {"_id":&state.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "sandbox_id":state.seal.target.sandbox_id.as_str(),"agent_id":state.seal.target.agent_id.as_str(),
        "record":record_bson(state).map_err(|_| AllocationError::Corrupt)?},
    )
}
pub(in crate::store) fn read_removal(
    row: &Document,
) -> Result<ChannelReuseRemovalState, AllocationError> {
    let state: ChannelReuseRemovalState = read_record(row).map_err(|_| AllocationError::Corrupt)?;
    let mut expected = removal_document(&state)?;
    if row.get_bool("audit").unwrap_or(false) {
        expected.insert("_id", audit_id(&state));
    }
    if expected
        .iter()
        .any(|(key, value)| row.get(key) != Some(value))
    {
        return Err(AllocationError::Corrupt);
    }
    Ok(state)
}
fn audit_id(state: &ChannelReuseRemovalState) -> String {
    format!("{}:revision:{}", state.id, state.revision)
}
async fn append_audit(
    database: &Database,
    state: &ChannelReuseRemovalState,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let mut row = removal_document(state)?;
    row.insert("_id", audit_id(state));
    row.insert("audit", true);
    majority_collection::<Document>(database, REMOVAL_AUDITS)
        .insert_one(row)
        .session(session)
        .await?;
    Ok(())
}
