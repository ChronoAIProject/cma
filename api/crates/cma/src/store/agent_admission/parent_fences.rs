use super::*;

pub(in crate::store) async fn advance_workspace(
    store: &MongoSandboxStore,
    previous: &layer2_core::sandbox::Workspace,
    next: &layer2_core::sandbox::Workspace,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let advanced = store
        .workspaces
        .update_one(
            crate::store::work_schema::query(doc! {
                "_id": previous.id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "channel_reuse_cleanup_execution": { "$exists": false },
                "owned_workspace_cleanup_admission_v1": { "$exists": false },
                "revision": signed(previous.revision, "workspace revision")
                    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                "next_worktree_number": signed(
                    previous.next_worktree_number,
                    "workspace next worktree number",
                )
                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
            }),
            crate::store::work_lease_update::replacement(
                workspace_replacement_fields(next).map_err(ApplyError::Domain)?,
            ),
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if advanced.matched_count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}

pub(in crate::store) async fn advance_sandbox(
    store: &MongoSandboxStore,
    previous: &layer2_core::sandbox::Sandbox,
    next: &layer2_core::sandbox::Sandbox,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let advanced = crate::store::work_lease_update::with_fence(
        &store.sandboxes,
        doc! {
            "_id": previous.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "channel_reuse_cleanup_execution": { "$exists": false },
            "owned_workspace_cleanup_admission_v1": { "$exists": false },
            crate::store::native_retirement_action::FIELD: { "$exists": false },
            "revision": signed(previous.revision, "sandbox revision")
                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        },
        Some(sandbox_replacement_fields(next).map_err(ApplyError::Domain)?),
        "workspace_admission_fence",
        session,
    )
    .await
    .map_err(ApplyError::Driver)?;
    if advanced.matched_count == 1 {
        crate::store::sandbox_usage::save::record_saved(store, session, next, previous.revision)
            .await?;
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}
