use super::*;
use crate::sandbox::channel_reuse::state::ChannelReuseState;
use crate::store::{
    agent_provenance, agents::read_agent, workspaces::read_workspace, worktrees::read_worktree,
};

pub(super) async fn read_owned(
    store: &MongoBridgeAllocationStore,
    permit: &ChannelReuseCleanupAuthority,
    allocation_row: &Document,
    session: &mut ClientSession,
) -> Result<(ChannelReuseSnapshot, BTreeSet<ChannelReuseCleanupHold>), MutationError> {
    let allocation = permit.allocation();
    let target = allocation.placement.target();
    let database = store.bridges.client.database(&store.records.namespace().db);
    let workspace_row = read(
        &database,
        "workspaces",
        target.workspace_id.as_str(),
        session,
    )
    .await?;
    let tree_row = read(&database, "worktrees", target.worktree_id.as_str(), session).await?;
    let agent_row = read(&database, "agents", target.agent_id.as_str(), session).await?;
    let creation_row = read(
        &database,
        "agent_creation_provenance",
        target.agent_id.as_str(),
        session,
    )
    .await?;
    let snapshot = ChannelReuseSnapshot {
        allocation: allocation.clone(),
        sandbox: permit.parent().lineage.sandbox.clone(),
        state: decode::<ChannelReuseState>(allocation_row, "channel_reuse")?,
        workspace: Some(read_workspace(&workspace_row).map_err(|_| AllocationError::Corrupt)?),
        worktree: Some(read_worktree(&tree_row).map_err(|_| AllocationError::Corrupt)?),
        agent: Some(read_agent(&agent_row).map_err(|_| AllocationError::Corrupt)?),
        creation: Some(
            agent_provenance::read(&creation_row).map_err(|_| AllocationError::Corrupt)?,
        ),
        workspace_creation: workspace_row
            .get("channel_reuse_creation")
            .map(|value| {
                mongodb::bson::deserialize_from_bson(value.clone())
                    .map_err(|_| AllocationError::Corrupt)
            })
            .transpose()?,
    };
    let seal = ChannelReuseCleanupSeal::from_snapshot(&snapshot)?;
    let holds = retained_inventory(&database, &seal, session).await?;
    // Only the exact owned rows are written. Unrelated siblings never acquire
    // a cleanup marker, lifecycle mutation, home operation or history deletion.
    for (name, row) in [
        ("workspaces", workspace_row),
        ("worktrees", tree_row),
        ("agents", agent_row),
        ("agent_creation_provenance", creation_row),
    ] {
        fence_row(&database, name, row, session).await?;
    }
    Ok((snapshot, holds))
}

async fn retained_inventory(
    database: &Database,
    seal: &ChannelReuseCleanupSeal,
    session: &mut ClientSession,
) -> Result<BTreeSet<ChannelReuseCleanupHold>, MutationError> {
    let mut holds = BTreeSet::new();
    let target = &seal.target;
    // Include historical rows and both indexed/embedded backreferences.
    // A misindexed sibling or already-deleted extra child still prevents an
    // assertion that the whole Workspace has one allocation-owned inventory.
    let filters = [
        (
            "agents",
            doc! {"$or":[{"workspace_id":target.workspace_id.as_str()},
            {"record.workspace_id":target.workspace_id.as_str()},
            {"worktree_id":target.worktree_id.as_str()},
            {"record.worktree_id":target.worktree_id.as_str()},
            {"record.creation_key":format!("channel-allocation:{}",seal.allocation_id)}]},
        ),
        (
            "worktrees",
            doc! {"$or":[{"workspace_id":target.workspace_id.as_str()},
            {"record.workspace_id":target.workspace_id.as_str()},
            {"record.agent_id":target.agent_id.as_str()}, {"record.path":&seal.worktree_path}]},
        ),
    ];
    for (name, filter) in filters {
        let collection: Collection<Document> = majority_collection(database, name);
        let mut cursor = collection
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(257)
            .session(&mut *session)
            .await?;
        let mut count = 0;
        while cursor.advance(&mut *session).await? {
            count += 1;
            if count > 256 {
                holds.insert(ChannelReuseCleanupHold::InventoryLimit);
                break;
            }
            let row = cursor.deserialize_current()?;
            let foreign = if name == "agents" {
                let agent = read_agent(&row).map_err(|_| AllocationError::Corrupt)?;
                if agent.id == target.agent_id {
                    false
                } else {
                    if agent.worktree_id == target.worktree_id
                        || agent.creation_key
                            == format!("channel-allocation:{}", seal.allocation_id)
                        || agent.workspace_id != target.workspace_id
                    {
                        return Err(AllocationError::IdentityMismatch.into());
                    }
                    true
                }
            } else {
                let tree = read_worktree(&row).map_err(|_| AllocationError::Corrupt)?;
                if tree.id == target.worktree_id {
                    false
                } else {
                    if tree.agent_id.as_ref() == Some(&target.agent_id)
                        || tree.path == seal.worktree_path
                        || tree.workspace_id != target.workspace_id
                    {
                        return Err(AllocationError::IdentityMismatch.into());
                    }
                    true
                }
            };
            if foreign && seal.workspace_owned {
                holds.insert(ChannelReuseCleanupHold::OtherWorkspaceChildren);
            }
        }
    }
    Ok(holds)
}

fn decode<T: serde::de::DeserializeOwned>(row: &Document, key: &str) -> Result<T, AllocationError> {
    mongodb::bson::deserialize_from_bson(row.get(key).cloned().ok_or(AllocationError::Corrupt)?)
        .map_err(|_| AllocationError::Corrupt)
}
