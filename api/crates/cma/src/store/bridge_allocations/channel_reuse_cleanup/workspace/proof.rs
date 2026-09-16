use super::*;
use crate::sandbox::owned_workspace_cleanup::WorkspaceCleanupState;

pub(super) async fn retained(
    database: &Database,
    allocation: &AllocationRecord,
    sandbox: &Sandbox,
    session: &mut ClientSession,
) -> Result<Option<(WorkspaceCleanupJob, WorkspaceCleanupState, Workspace)>, JobError> {
    let allocation_row = read(database, "bridge_allocations", &allocation.id, session).await?;
    let workspace_row = majority_collection::<Document>(database, "workspaces")
        .find_one(doc! {"_id":allocation.placement.target().workspace_id.as_str()})
        .session(&mut *session)
        .await?;
    let mut rows = majority_collection::<Document>(database, WORKSPACE_JOBS)
        .find(doc! {"$or":[{"allocation_id":&allocation.id},{"record.admission.receipt.allocation_id":&allocation.id}]})
        .limit(2).session(&mut *session).await?;
    let job_row = if rows.advance(&mut *session).await? {
        let row: Document = rows.deserialize_current()?;
        if rows.advance(&mut *session).await? {
            return Err(CleanupError::Corrupt.into());
        }
        Some(row)
    } else {
        None
    };
    let Some(job_row) = job_row else {
        if allocation_row.contains_key(WORKSPACE_JOB_FIELD)
            || workspace_row.as_ref().is_some_and(|row| {
                row.contains_key(WORKSPACE_JOB_FIELD) || row.contains_key(WORKSPACE_ADMISSION_FIELD)
            })
        {
            return Err(CleanupError::Corrupt.into());
        }
        for collection in [WORKSPACE_JOB_AUDITS, WORKSPACE_TOMBSTONES] {
            if majority_collection::<Document>(database, collection)
                .find_one(doc! {"$or":[{"allocation_id":&allocation.id},{"record.admission.receipt.allocation_id":&allocation.id},{"record.receipt.allocation_id":&allocation.id}]})
                .session(&mut *session).await?.is_some()
            { return Err(CleanupError::Corrupt.into()); }
        }
        return Ok(None);
    };
    let job = records::read_job(&job_row)?;
    let admission = &job.admission;
    let workspace_row = workspace_row.ok_or(CleanupError::Corrupt)?;
    if admission.receipt.allocation_id != allocation.id
        || admission.receipt.bridge_id != allocation.identity.bridge_id
        || admission.receipt.generation != allocation.identity.generation
        || admission.receipt.target != *allocation.placement.target()
        || admission.owner != allocation.owner
        || admission.allocation_sha256 != digest(allocation)?
        || allocation_row.get_document(WORKSPACE_JOB_FIELD).ok()
            != Some(&records::marker(admission))
        || workspace_row.get_document(WORKSPACE_JOB_FIELD).ok() != Some(&records::marker(admission))
    {
        return Err(CleanupError::Corrupt.into());
    }
    let consent_row = read(
        database,
        WORKSPACE_JOB_AUDITS,
        &admission.receipt.id,
        session,
    )
    .await?;
    if records::read_consent(&consent_row)? != *admission {
        return Err(CleanupError::Corrupt.into());
    }
    let state_row = read(
        database,
        WORKSPACE_TOMBSTONES,
        &admission.receipt.id,
        session,
    )
    .await?;
    let state = records::read_workspace_cleanup_state(&state_row)?;
    records::require_audit_chain(database, &state, session).await?;
    if state.admission != *admission || job.progress > state.progress {
        return Err(CleanupError::Corrupt.into());
    }
    let (_, child) = child::complete(
        database,
        allocation,
        &admission.receipt.request.child_cleanup_receipt_id,
        session,
    )
    .await?;
    if child != state.child {
        return Err(CleanupError::Corrupt.into());
    }
    let terminal = state.progress == WorkspaceCleanupProgress::Complete;
    let (workspace, holds) =
        ownership::require(database, allocation, sandbox, &child, terminal, session).await?;
    if state.progress >= WorkspaceCleanupProgress::AdmissionClosed {
        if !holds.is_empty()
            || workspace_row.get_document(WORKSPACE_ADMISSION_FIELD).ok()
                != Some(&records::marker(admission))
        {
            return Err(CleanupError::Corrupt.into());
        }
    } else if workspace_row.contains_key(WORKSPACE_ADMISSION_FIELD) {
        return Err(CleanupError::Corrupt.into());
    }
    if state.progress >= WorkspaceCleanupProgress::RemovalStarted {
        if workspace_row.get_str(CLEANUP_EXECUTION_FIELD).ok() != Some(state.id()) {
            return Err(CleanupError::Corrupt.into());
        }
        let parent = read(database, "sandboxes", sandbox.id.as_str(), session).await?;
        let excluded = parent.get_str(CLEANUP_EXECUTION_FIELD).ok() == Some(state.id());
        if terminal == excluded
            || (terminal && state.terminal_workspace_revision != Some(workspace.revision))
        {
            return Err(CleanupError::Corrupt.into());
        }
        fence_row(database, "sandboxes", parent, session).await?;
    }
    for (name, row) in [
        (WORKSPACE_JOBS, job_row),
        (WORKSPACE_JOB_AUDITS, consent_row),
        (WORKSPACE_TOMBSTONES, state_row),
    ] {
        fence_row(database, name, row, session).await?;
    }
    Ok(Some((job, state, workspace)))
}

pub(super) async fn claimed(
    store: &MongoBridgeAllocationStore,
    permit: &WorkspaceCleanupAuthority,
    session: &mut ClientSession,
) -> Result<(WorkspaceCleanupJob, WorkspaceCleanupState, Workspace), JobError> {
    permit.require_current()?;
    super::super::authority::fence_common(store, permit.common(), session).await?;
    let database = store.bridges.client.database(&store.records.namespace().db);
    let value = retained(
        &database,
        permit.common().allocation(),
        &permit.common().parent().lineage.sandbox,
        session,
    )
    .await?
    .ok_or(CleanupError::Corrupt)?;
    if value.0.admission.receipt.id != permit.id()
        || value.0.admission.receipt.requested_by != *permit.common().management().actor()
    {
        return Err(CleanupError::Corrupt.into());
    }
    value.0.require_claim(
        permit.claim(),
        mongodb::bson::DateTime::now().timestamp_millis(),
    )?;
    permit.require_current()?;
    Ok(value)
}
