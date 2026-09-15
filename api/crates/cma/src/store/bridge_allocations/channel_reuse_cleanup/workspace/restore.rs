//! One transactional proof is shared by normal completion, capacity and restore.
use super::*;
use layer2_core::{resource_access::ResourceOwner, sandbox::store::RetiredWorkspaceRuntimePath};

pub(in crate::store) async fn verified_workspace_restore_paths(
    database: &Database,
    sandbox: &Sandbox,
    owner: &ResourceOwner,
    session: &mut ClientSession,
) -> Result<Vec<RetiredWorkspaceRuntimePath>, MutationError> {
    paths(database, sandbox, owner, session)
        .await
        .map_err(core_error)
}
async fn paths(
    database: &Database,
    sandbox: &Sandbox,
    owner: &ResourceOwner,
    session: &mut ClientSession,
) -> Result<Vec<RetiredWorkspaceRuntimePath>, JobError> {
    let mut cursor = majority_collection::<Document>(database, WORKSPACE_TOMBSTONES)
        .find(doc! {"$or":[{"sandbox_id":sandbox.id.as_str()},
        {"record.admission.receipt.target.sandbox_id":sandbox.id.as_str()},
        {"record.child.seal.target.sandbox_id":sandbox.id.as_str()}]})
        .sort(doc! {"_id":1})
        .limit(257)
        .session(&mut *session)
        .await?;
    let mut result = Vec::new();
    let mut marked = std::collections::BTreeMap::new();
    let mut count = 0;
    while cursor.advance(&mut *session).await? {
        count += 1;
        if count > 256 {
            return Err(CleanupError::Corrupt.into());
        }
        let row: Document = cursor.deserialize_current()?;
        let state = records::read_workspace_cleanup_state(&row)?;
        let allocation_row = read(
            database,
            "bridge_allocations",
            &state.admission.receipt.allocation_id,
            session,
        )
        .await?;
        let allocation = super::super::super::records::decode_record(&allocation_row)?;
        let (_, proved, workspace) = proof::retained(database, &allocation, sandbox, session)
            .await?
            .ok_or(CleanupError::Corrupt)?;
        if proved != state
            || &state.admission.owner != owner
            || allocation.owner != *owner
            || state.child.seal.sandbox_created_at != sandbox.created_at
            || state.admission.receipt.target.sandbox_id != sandbox.id
        {
            return Err(CleanupError::Corrupt.into());
        }
        if state.progress >= WorkspaceCleanupProgress::AdmissionClosed
            && marked
                .insert(workspace.id.clone(), records::marker(&state.admission))
                .is_some()
        {
            return Err(CleanupError::Corrupt.into());
        }
        match state.progress {
            WorkspaceCleanupProgress::Complete => result.push(RetiredWorkspaceRuntimePath {
                workspace_id: workspace.id,
                workspace_name: workspace.name,
                workspace_created_at: workspace.created_at,
                cleanup_receipt_id: state.id().into(),
                cleanup_sha256: state.digest.clone(),
                removal_request_sha256: state.canonical_request().request_sha256,
            }),
            WorkspaceCleanupProgress::Requested | WorkspaceCleanupProgress::AdmissionClosed => {}
            _ => return Err(CleanupError::Conflict.into()),
        }
    }
    let mut reverse = majority_collection::<Document>(database, "workspaces")
        .find(doc! {"$or":[{"sandbox_id":sandbox.id.as_str()},{"record.sandbox_id":sandbox.id.as_str()}], WORKSPACE_ADMISSION_FIELD:{"$exists":true}})
        .sort(doc! {"_id":1}).limit(257).session(&mut *session).await?;
    let mut count = 0;
    while reverse.advance(&mut *session).await? {
        count += 1;
        if count > 256 {
            return Err(CleanupError::Corrupt.into());
        }
        let row: Document = reverse.deserialize_current()?;
        let workspace =
            crate::store::workspaces::read_workspace(&row).map_err(|_| CleanupError::Corrupt)?;
        let marker = row
            .get_document(WORKSPACE_ADMISSION_FIELD)
            .map_err(|_| CleanupError::Corrupt)?;
        if Some(marker) != marked.remove(&workspace.id).as_ref() {
            return Err(CleanupError::Corrupt.into());
        }
        fence_row(database, "workspaces", row, session).await?;
    }
    if !marked.is_empty() {
        return Err(CleanupError::Corrupt.into());
    }
    Ok(result)
}
