use super::*;
use crate::sandbox::channel_reuse::children::ReuseWorkspaceCreation;
use crate::store::workspaces::read_workspace;
use layer2_core::bridge::{
    allocation::{AllocationPlacement, AllocationState},
    BridgePlacementPolicy,
};

pub(super) async fn require(
    database: &Database,
    allocation: &AllocationRecord,
    sandbox: &Sandbox,
    child: &ChannelReuseRemovalState,
    terminal: bool,
    session: &mut ClientSession,
) -> Result<(Workspace, Vec<WorkspaceCleanupHold>), JobError> {
    allocation.validate()?;
    let AllocationPlacement::ReuseParent { parent, .. } = &allocation.placement else {
        return Err(CleanupError::Conflict.into());
    };
    if allocation.state != AllocationState::Retired
        || !matches!(
            allocation.config.placement,
            BridgePlacementPolicy::WorkspacePerPartition { .. }
        )
        || parent.workspace_created_at.is_some()
        || !child.seal.workspace_owned
        || child.seal.owner != allocation.owner
        || child.seal.allocation_id != allocation.id
        || child.seal.allocation_sha256 != digest(allocation)?
        || child.seal.sandbox_created_at != sandbox.created_at
    {
        return Err(CleanupError::Conflict.into());
    }
    let target = allocation.placement.target();
    let row = read(
        database,
        "workspaces",
        target.workspace_id.as_str(),
        session,
    )
    .await?;
    let workspace = read_workspace(&row).map_err(|_| CleanupError::Corrupt)?;
    let creation: ReuseWorkspaceCreation = mongodb::bson::deserialize_from_bson(
        row.get("channel_reuse_creation")
            .cloned()
            .ok_or(CleanupError::Corrupt)?,
    )
    .map_err(|_| CleanupError::Corrupt)?;
    creation.require_identity(allocation, sandbox, &workspace)?;
    if child.seal.workspace_creation_sha256.as_ref() != Some(&digest(&creation)?)
        || child.seal.workspace_created_at != workspace.created_at
        || child.seal.workspace_path != workspace.path
        || terminal != (workspace.status == WorkspaceStatus::Removed)
        || terminal != workspace.removed_at.is_some()
        || (!terminal && workspace.status != WorkspaceStatus::Ready)
    {
        return Err(CleanupError::Corrupt.into());
    }
    let mut holds = Vec::new();
    if workspace.work.step != WorkspaceWorkStep::None
        || workspace.work.lease.is_some()
        || workspace.work.execution.is_some()
    {
        holds.push(WorkspaceCleanupHold::WorkspaceBusy);
    }
    let inventory =
        super::super::inventory::retained_inventory(database, &child.seal, session).await?;
    for hold in inventory {
        holds.push(match hold {
            ChannelReuseCleanupHold::OtherWorkspaceChildren => {
                WorkspaceCleanupHold::OtherWorkspaceChildren
            }
            ChannelReuseCleanupHold::InventoryLimit => WorkspaceCleanupHold::InventoryLimit,
            _ => return Err(CleanupError::Corrupt.into()),
        });
    }
    // A retained different-ID name/path or creation alias cannot be scrubbed.
    let mut aliases = majority_collection::<Document>(database, "workspaces").find(doc! {
        "$or":[{"_id":target.workspace_id.as_str()},
        {"channel_reuse_creation.allocation_id":&allocation.id},
        {"channel_reuse_creation.workspace_id":target.workspace_id.as_str()},
        {"$and":[{"$or":[{"sandbox_id":target.sandbox_id.as_str()},{"record.sandbox_id":target.sandbox_id.as_str()}]},
        {"$or":[{"name":&workspace.name},{"record.name":&workspace.name},{"path":&workspace.path},{"record.path":&workspace.path}]}]}]
    }).limit(257).session(&mut *session).await?;
    let mut count = 0;
    while aliases.advance(&mut *session).await? {
        count += 1;
        let candidate: Document = aliases.deserialize_current()?;
        if count != 1 || read_workspace(&candidate).map_err(|_| CleanupError::Corrupt)? != workspace
        {
            return Err(CleanupError::Corrupt.into());
        }
    }
    if count != 1 {
        return Err(CleanupError::Corrupt.into());
    }
    let mut reservations = majority_collection::<Document>(database, "bridge_allocations")
        .find(doc! {
            "$or":[{"record.placement.target.workspace_id":target.workspace_id.as_str()},
            {"record.receipt.target.workspace_id":target.workspace_id.as_str()},
            {"record.config.placement.parent.workspace_id":target.workspace_id.as_str()}]
        })
        .limit(257)
        .session(&mut *session)
        .await?;
    let mut count = 0;
    while reservations.advance(&mut *session).await? {
        count += 1;
        if count > 256 {
            holds.push(WorkspaceCleanupHold::InventoryLimit);
            break;
        }
        let candidate: Document = reservations.deserialize_current()?;
        let retained = super::super::super::records::decode_record(&candidate)?;
        if retained.id != allocation.id {
            holds.push(WorkspaceCleanupHold::OtherWorkspaceChildren);
        }
        fence_row(database, "bridge_allocations", candidate, session).await?;
    }
    if count == 0 {
        return Err(CleanupError::Corrupt.into());
    }
    holds.extend(creation_inventory::holds(database, sandbox, child, session).await?);
    holds.sort();
    holds.dedup();
    fence_row(database, "workspaces", row, session).await?;
    Ok((workspace, holds))
}
