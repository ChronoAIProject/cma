//! Workspace removal admission under a Mongo transaction.

use futures_util::TryStreamExt as _;
use layer2_core::ids::{SubjectId, WorkspaceId};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Workspace, WorkspaceStatus, WorkspaceWorkStep};
use mongodb::bson::doc;

use super::agents::read_agent;
use super::sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::sandboxes::read_sandbox;
use super::workspace_admission::{lock_sandbox, ApplyError};
use super::workspaces::{read_workspace, workspace_replacement_fields};

pub(super) async fn apply_removal(
    store: &MongoSandboxStore,
    id: &WorkspaceId,
    owner: &SubjectId,
    now: &str,
    session: &mut mongodb::ClientSession,
) -> Result<Workspace, ApplyError> {
    let document = store
        .workspaces
        .find_one(doc! { "_id": id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let mut workspace = read_workspace(&document).map_err(ApplyError::Domain)?;
    if &workspace.owner != owner {
        return Err(ApplyError::Domain(SandboxStoreError::NotFound));
    }
    if matches!(
        workspace.status,
        WorkspaceStatus::Removing | WorkspaceStatus::Removed
    ) {
        return Ok(workspace);
    }
    lock_sandbox(store, workspace.sandbox_id.as_str(), session).await?;
    let parent = store
        .sandboxes
        .find_one(doc! { "_id": workspace.sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::SandboxNotReady))?;
    if parent.owner != *owner
        || parent.lifecycle.checkpoint_capture.is_some()
        || parent.status == layer2_core::sandbox::SandboxStatus::Rotating
    {
        return Err(ApplyError::Domain(SandboxStoreError::SandboxNotReady));
    }

    let expected = workspace.revision;
    lock_workspace(store, id, expected, session).await?;
    validate_agents(store, &workspace, session).await?;
    workspace
        .transition(WorkspaceStatus::Removing, now.to_string(), None)
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    workspace
        .work
        .schedule(WorkspaceWorkStep::Remove, now.to_string());
    let updated = store
        .workspaces
        .update_one(
            workspace_revision_filter(id, expected)?,
            doc! { "$set": workspace_replacement_fields(&workspace)
            .map_err(ApplyError::Domain)? },
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if updated.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(workspace)
}

async fn lock_workspace(
    store: &MongoSandboxStore,
    id: &WorkspaceId,
    revision: u64,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let locked = store
        .workspaces
        .update_one(
            workspace_revision_filter(id, revision)?,
            doc! { "$inc": { "removal_lock_fence": 1_i64 } },
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if locked.matched_count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::Conflict))
    }
}

async fn validate_agents(
    store: &MongoSandboxStore,
    workspace: &Workspace,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let mut cursor = store
        .agents
        .find(doc! { "workspace_id": workspace.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let documents = cursor
        .stream(&mut *session)
        .try_collect::<Vec<_>>()
        .await
        .map_err(ApplyError::Driver)?;
    for document in documents {
        let agent = read_agent(&document).map_err(ApplyError::Domain)?;
        if agent.workspace_id != workspace.id
            || agent.owner != workspace.owner
            || agent.sandbox_id != workspace.sandbox_id
        {
            return Err(ApplyError::Domain(SandboxStoreError::Corrupt(
                "workspace child lineage is inconsistent".into(),
            )));
        }
    }
    Ok(())
}

fn workspace_revision_filter(
    id: &WorkspaceId,
    revision: u64,
) -> Result<mongodb::bson::Document, ApplyError> {
    Ok(doc! {
        "_id": id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "channel_reuse_cleanup_execution": { "$exists": false },
                "owned_workspace_cleanup_admission_v1": { "$exists": false },
        "revision": signed(revision, "workspace removal revision")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
    })
}
