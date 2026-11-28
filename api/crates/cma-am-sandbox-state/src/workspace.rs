//! BSON records and indexes for Workspace aggregates.

use cma_am_mongo_primitives::{
    i64_field_matches, nullable, record_bson, signed, string_field_matches,
};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Workspace, WorkspaceOrigin};
use mongodb::bson::{doc, Bson, Document};
use mongodb::{options::IndexOptions, IndexModel};

pub fn workspace_indexes() -> Vec<IndexModel> {
    vec![
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1, "name": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_sandbox_name_live".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "removed_at": Bson::Null })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1, "origin_index": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_sandbox_profile_origin".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "origin_type": "sandbox_profile" })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1, "origin_launch_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_sandbox_trigger_origin".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "origin_type": "trigger" })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1, "origin_operation_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_sandbox_lifecycle_origin".to_string())
                    .unique(true)
                    .partial_filter_expression(doc! { "origin_type": "lifecycle" })
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! { "work_pending": 1, "work_ready_at": 1, "work_lease_expires_at": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_due_work".to_string())
                    .build(),
            )
            .build(),
        // Maintenance fences every child, including removed and manual Workspaces.
        // None of the partial indexes above covers that complete-child lookup.
        IndexModel::builder()
            .keys(doc! { "sandbox_id": 1 })
            .options(
                IndexOptions::builder()
                    .name("workspaces_sandbox".to_string())
                    .build(),
            )
            .build(),
    ]
}

pub fn workspace_document(workspace: &Workspace) -> Result<Document, SandboxStoreError> {
    let (origin_type, origin_index, origin_launch_id) = origin_fields(&workspace.origin);
    let lease_expires_at = workspace.work.lease.as_ref().map(|lease| &lease.expires_at);
    Ok(doc! {
        "_id": workspace.id.as_str(),
        "document_schema_version": super::work_schema::version(workspace.work.authority_protocol, false),
        "sandbox_id": workspace.sandbox_id.as_str(),
        "owner": workspace.owner.as_str(),
        "name": &workspace.name,
        "origin_type": origin_type,
        "origin_index": origin_index,
        "origin_launch_id": origin_launch_id,
        "origin_operation_id": operation_origin(&workspace.origin),
        "status": record_bson(&workspace.status).map_err(SandboxStoreError::Corrupt)?,
        "removed_at": nullable(workspace.removed_at.as_ref()),
        "work_pending": workspace.work.step.is_pending(),
        "work_ready_at": &workspace.work.ready_at,
        "work_lease_expires_at": nullable(lease_expires_at),
        "next_worktree_number": signed(
            workspace.next_worktree_number,
            "workspace next worktree number",
        )
        .map_err(SandboxStoreError::Corrupt)?,
        "revision": signed(workspace.revision, "workspace revision")
            .map_err(SandboxStoreError::Corrupt)?,
        "record": record_bson(workspace).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub fn read_workspace(document: &Document) -> Result<Workspace, SandboxStoreError> {
    let workspace: Workspace =
        super::work_schema::read(document).map_err(SandboxStoreError::Corrupt)?;
    let status = record_bson(&workspace.status).map_err(SandboxStoreError::Corrupt)?;
    let (origin_type, origin_index, origin_launch_id) = origin_fields(&workspace.origin);
    let revision =
        signed(workspace.revision, "workspace revision").map_err(SandboxStoreError::Corrupt)?;
    let next_worktree_number = signed(
        workspace.next_worktree_number,
        "workspace next worktree number",
    )
    .map_err(SandboxStoreError::Corrupt)?;
    if !string_field_matches(document, "_id", workspace.id.as_str())
        || !string_field_matches(document, "sandbox_id", workspace.sandbox_id.as_str())
        || !string_field_matches(document, "owner", workspace.owner.as_str())
        || !string_field_matches(document, "name", &workspace.name)
        || !string_field_matches(document, "origin_type", origin_type)
        || document.get("origin_index") != Some(&origin_index)
        || document.get("origin_launch_id") != Some(&origin_launch_id)
        || document.get("origin_operation_id").unwrap_or(&Bson::Null)
            != &operation_origin(&workspace.origin)
        || document.get("status") != Some(&status)
        || document.get("removed_at") != Some(&nullable(workspace.removed_at.as_ref()))
        || document.get_bool("work_pending").ok() != Some(workspace.work.step.is_pending())
        || !string_field_matches(document, "work_ready_at", &workspace.work.ready_at)
        || document.get("work_lease_expires_at")
            != Some(&nullable(
                workspace.work.lease.as_ref().map(|lease| &lease.expires_at),
            ))
        || !i64_field_matches(document, "next_worktree_number", next_worktree_number)
        || !i64_field_matches(document, "revision", revision)
    {
        return Err(SandboxStoreError::Corrupt(
            "workspace query fields disagree with its aggregate".into(),
        ));
    }
    Ok(workspace)
}

pub fn workspace_replacement_fields(workspace: &Workspace) -> Result<Document, SandboxStoreError> {
    let mut document = workspace_document(workspace)?;
    document.remove("_id");
    Ok(document)
}

fn origin_fields(origin: &WorkspaceOrigin) -> (&'static str, Bson, Bson) {
    match origin {
        WorkspaceOrigin::Manual => ("manual", Bson::Null, Bson::Null),
        WorkspaceOrigin::SandboxProfile { index } => (
            "sandbox_profile",
            Bson::Int64(i64::from(*index)),
            Bson::Null,
        ),
        WorkspaceOrigin::Trigger { launch_id } => {
            ("trigger", Bson::Null, Bson::String(launch_id.clone()))
        }
        WorkspaceOrigin::Lifecycle { .. } => ("lifecycle", Bson::Null, Bson::Null),
    }
}

fn operation_origin(origin: &WorkspaceOrigin) -> Bson {
    match origin {
        WorkspaceOrigin::Lifecycle { operation_id } => Bson::String(operation_id.clone()),
        _ => Bson::Null,
    }
}
