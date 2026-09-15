use super::*;
use crate::sandbox::owned_workspace_cleanup::WorkspaceCleanupState;

pub(super) fn job_document(job: &WorkspaceCleanupJob) -> Result<Document, CleanupError> {
    job.validate()?;
    Ok(
        doc! {"_id":&job.admission.receipt.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "allocation_id":&job.admission.receipt.allocation_id,
        "workspace_id":job.admission.receipt.target.workspace_id.as_str(),
        "complete":job.progress == WorkspaceCleanupProgress::Complete,
        "next_attempt_at_ms":job.next_attempt_at_ms,
        "lease_expires_at_ms":job.lease.as_ref().map(|lease| lease.expires_at_ms),
        "record":record_bson(job).map_err(|_| CleanupError::Corrupt)?},
    )
}
pub(super) fn read_job(row: &Document) -> Result<WorkspaceCleanupJob, CleanupError> {
    let job = read_record(row).map_err(|_| CleanupError::Corrupt)?;
    exact(row, &job_document(&job)?)?;
    Ok(job)
}
pub(super) fn consent_document(
    admission: &WorkspaceCleanupJobAdmission,
) -> Result<Document, CleanupError> {
    admission.validate()?;
    Ok(
        doc! {"_id":&admission.receipt.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "allocation_id":&admission.receipt.allocation_id,
        "workspace_id":admission.receipt.target.workspace_id.as_str(),
        "record":record_bson(admission).map_err(|_| CleanupError::Corrupt)?},
    )
}
pub(super) fn read_consent(row: &Document) -> Result<WorkspaceCleanupJobAdmission, CleanupError> {
    let admission = read_record(row).map_err(|_| CleanupError::Corrupt)?;
    exact(row, &consent_document(&admission)?)?;
    Ok(admission)
}
pub(super) fn state_document(state: &WorkspaceCleanupState) -> Result<Document, CleanupError> {
    state.validate()?;
    Ok(
        doc! {"_id":state.id(),"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "allocation_id":&state.admission.receipt.allocation_id,
        "workspace_id":state.admission.receipt.target.workspace_id.as_str(),
        "sandbox_id":state.admission.receipt.target.sandbox_id.as_str(),
        "complete":state.progress == WorkspaceCleanupProgress::Complete,
        "record":record_bson(state).map_err(|_| CleanupError::Corrupt)?},
    )
}
pub(crate) fn read_workspace_cleanup_state(
    row: &Document,
) -> Result<WorkspaceCleanupState, CleanupError> {
    let state = read_record(row).map_err(|_| CleanupError::Corrupt)?;
    let mut expected = state_document(&state)?;
    if row.get_bool("audit").unwrap_or(false) {
        expected.insert("_id", audit_id(&state));
    }
    exact(row, &expected)?;
    Ok(state)
}
pub(super) fn audit_id(state: &WorkspaceCleanupState) -> String {
    format!("{}:revision:{}", state.id(), state.revision)
}
pub(super) async fn append_audit(
    database: &Database,
    state: &WorkspaceCleanupState,
    session: &mut ClientSession,
) -> Result<(), JobError> {
    let mut row = state_document(state)?;
    row.insert("_id", audit_id(state));
    row.insert("audit", true);
    majority_collection::<Document>(database, WORKSPACE_AUDITS)
        .insert_one(row)
        .session(session)
        .await?;
    Ok(())
}
pub(super) fn marker(admission: &WorkspaceCleanupJobAdmission) -> Document {
    doc! {"id":&admission.receipt.id,"digest":&admission.digest}
}
fn exact(row: &Document, expected: &Document) -> Result<(), CleanupError> {
    if expected
        .iter()
        .any(|(key, value)| row.get(key) != Some(value))
    {
        return Err(CleanupError::Corrupt);
    }
    Ok(())
}

// Every physical stage remains independently retained and ordered.
pub(super) async fn require_audit_chain(
    database: &Database,
    state: &WorkspaceCleanupState,
    session: &mut ClientSession,
) -> Result<(), JobError> {
    let mut previous: Option<WorkspaceCleanupState> = None;
    for revision in 1..=state.revision {
        let row = read(
            database,
            WORKSPACE_AUDITS,
            &format!("{}:revision:{revision}", state.id()),
            session,
        )
        .await?;
        let audited = read_workspace_cleanup_state(&row)?;
        if audited.revision != revision
            || audited.admission != state.admission
            || audited.child != state.child
            || (revision == state.revision && audited != *state)
            || previous.as_ref().is_some_and(|previous| {
                audited.updated_at_ms < previous.updated_at_ms
                    || (previous.runtime.is_some() && audited.runtime != previous.runtime)
                    || (previous.removal.is_some() && audited.removal != previous.removal)
                    || (previous.observation.is_some()
                        && audited.observation != previous.observation)
            })
        {
            return Err(CleanupError::Corrupt.into());
        }
        fence_row(database, WORKSPACE_AUDITS, row, session).await?;
        previous = Some(audited);
    }
    Ok(())
}
