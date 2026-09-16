//! Read-only stage predicates retain every original child receipt and marker.
use super::*;

pub(super) async fn complete(
    database: &Database,
    allocation: &AllocationRecord,
    receipt_id: &str,
    session: &mut ClientSession,
) -> Result<(CleanupJob, ChannelReuseRemovalState), JobError> {
    let allocation_row = read(database, "bridge_allocations", &allocation.id, session).await?;
    if super::super::super::records::decode_record(&allocation_row)? != *allocation {
        return Err(CleanupError::Corrupt.into());
    }
    let job_row = read(
        database,
        "channel_child_cleanup_jobs_v1",
        receipt_id,
        session,
    )
    .await?;
    let job = super::super::jobs::read_job(&job_row)?;
    let acceptance = &job.admission;
    let audit = read(
        database,
        "channel_child_cleanup_job_admissions_v1",
        receipt_id,
        session,
    )
    .await?;
    if super::super::jobs::read_admission(&audit)? != *acceptance
        || job.progress != ChildCleanupProgress::Complete
        || acceptance.receipt.id != receipt_id
        || acceptance.receipt.allocation_id != allocation.id
        || acceptance.receipt.bridge_id != allocation.identity.bridge_id
        || acceptance.receipt.generation != allocation.identity.generation
        || acceptance.receipt.request.expected_allocation_revision != allocation.revision
        || acceptance.owner != allocation.owner
        || acceptance.allocation_sha256 != digest(allocation)?
        || acceptance.receipt.target != *allocation.placement.target()
        || allocation
            .placement
            .profiles()
            .is_none_or(|profiles| profiles.approved_by != acceptance.receipt.requested_by)
        || allocation_row
            .get_document("channel_child_cleanup_job_v1")
            .ok()
            != Some(&doc! {"id":receipt_id,"digest":&acceptance.digest})
    {
        return Err(CleanupError::Corrupt.into());
    }
    let saved_row = read(database, REQUESTS, &acceptance.cleanup_request_id, session).await?;
    let saved = read_state(&saved_row)?;
    let saved_audit = read(database, AUDITS, &saved.id, session).await?;
    if read_state(&saved_audit)? != saved
        || saved.digest != acceptance.cleanup_request_sha256
        || saved.seal.allocation_sha256 != acceptance.allocation_sha256
        || saved.requested_by != acceptance.receipt.requested_by
        || saved.seal.owner != allocation.owner
        || saved.seal.target != acceptance.receipt.target
        || saved.request.expected_allocation_revision != allocation.revision
        || allocation_row
            .get_document("channel_reuse_cleanup_request")
            .ok()
            != Some(&doc! {"id":&saved.id,"digest":&saved.digest})
    {
        return Err(CleanupError::Corrupt.into());
    }
    let closure_id = ChannelReuseCleanupAdmission::id(&saved.id);
    let closure_row = read(
        database,
        "channel_reuse_cleanup_admissions",
        &closure_id,
        session,
    )
    .await?;
    let closure = super::super::drain::read_admission(&closure_row, &saved)?;
    let closure_audit = read(
        database,
        "channel_reuse_cleanup_admission_audits",
        &closure_id,
        session,
    )
    .await?;
    let agent = read(
        database,
        "agents",
        saved.seal.target.agent_id.as_str(),
        session,
    )
    .await?;
    if super::super::drain::read_admission(&closure_audit, &saved)? != closure
        || agent.get_document(CLEANUP_ADMISSION_FIELD).ok()
            != Some(&doc! {"id":&closure.id,"digest":&closure.digest})
    {
        return Err(CleanupError::Corrupt.into());
    }
    let id = format!("{}:child-removal:v1", saved.id);
    let removal_row = read(database, CLEANUP_TOMBSTONES, &id, session).await?;
    let removal = read_removal(&removal_row)?;
    let removal_audit = read(
        database,
        "channel_reuse_cleanup_removal_audits",
        &super::super::removal::audit_id(&removal),
        session,
    )
    .await?;
    if read_removal(&removal_audit)? != removal
        || removal.phase != ChannelReuseRemovalPhase::Complete
        || removal.request_sha256 != saved.digest
        || removal.admission_sha256 != closure.digest
        || removal.seal != saved.seal
    {
        return Err(CleanupError::Corrupt.into());
    }
    require_complete(database, &removal, session).await?;
    for revision in 1..removal.revision {
        let row = read(
            database,
            "channel_reuse_cleanup_removal_audits",
            &format!("{}:revision:{revision}", removal.id),
            session,
        )
        .await?;
        let previous = read_removal(&row)?;
        if previous.revision != revision
            || previous.seal != removal.seal
            || previous.runtime != removal.runtime
            || previous.request_sha256 != removal.request_sha256
            || previous.admission_sha256 != removal.admission_sha256
            || previous.authority_fence != removal.authority_fence
            || previous.updated_at_ms > removal.updated_at_ms
        {
            return Err(CleanupError::Corrupt.into());
        }
        fence_row(
            database,
            "channel_reuse_cleanup_removal_audits",
            row,
            session,
        )
        .await?;
    }

    // Keep creation and startup proof even after terminal lifecycle changes.
    let provenance_row = read(
        database,
        "agent_creation_provenance",
        saved.seal.target.agent_id.as_str(),
        session,
    )
    .await?;
    let provenance =
        crate::store::agent_provenance::read(&provenance_row).map_err(|_| CleanupError::Corrupt)?;
    let state: crate::sandbox::channel_reuse::state::ChannelReuseState =
        mongodb::bson::deserialize_from_bson(
            allocation_row
                .get("channel_reuse")
                .cloned()
                .ok_or(CleanupError::Corrupt)?,
        )
        .map_err(|_| CleanupError::Corrupt)?;
    if digest(&provenance)? != saved.seal.creation_sha256
        || digest(state.startup_plan.as_ref().ok_or(CleanupError::Corrupt)?)?
            != saved.seal.startup_sha256
    {
        return Err(CleanupError::Corrupt.into());
    }
    for (name, row) in [
        ("channel_child_cleanup_jobs_v1", job_row),
        ("channel_child_cleanup_job_admissions_v1", audit),
        (REQUESTS, saved_row),
        (AUDITS, saved_audit),
        ("channel_reuse_cleanup_admissions", closure_row),
        ("channel_reuse_cleanup_admission_audits", closure_audit),
        (CLEANUP_TOMBSTONES, removal_row),
        ("channel_reuse_cleanup_removal_audits", removal_audit),
        ("agent_creation_provenance", provenance_row),
    ] {
        fence_row(database, name, row, session).await?;
    }
    // Nested terminal proof fenced Agent/worktree; reread allocation at callers' CAS.
    fence_row(database, "bridge_allocations", allocation_row, session).await?;
    Ok((job, removal))
}
