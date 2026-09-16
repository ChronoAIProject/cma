//! Ordinary admission accounts for accepted Channel children before they exist.
//! Callers hold the existing parent transaction fence and handle replay first.
use std::collections::BTreeSet;

use layer2_core::{
    bridge::allocation::{AllocationError, AllocationPlacement},
    sandbox::store::SandboxStoreError,
    SandboxId, WorkspaceId,
};
use mongodb::{
    bson::{doc, Bson},
    ClientSession,
};

use super::{
    bridge_allocations::MutationError, sandbox_mongo::MongoSandboxStore, MongoBridgeAllocationStore,
};

pub(super) enum CapacityError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<mongodb::error::Error> for CapacityError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

impl From<SandboxStoreError> for CapacityError {
    fn from(error: SandboxStoreError) -> Self {
        Self::Domain(error)
    }
}

impl From<MutationError> for CapacityError {
    fn from(error: MutationError) -> Self {
        match error {
            MutationError::Mongo(error) => Self::Driver(error),
            MutationError::Core(AllocationError::Capacity) => {
                Self::Domain(SandboxStoreError::Unavailable(
                    "shared child reservation inventory is incomplete".into(),
                ))
            }
            MutationError::Core(AllocationError::Conflict) => {
                Self::Domain(SandboxStoreError::Conflict)
            }
            MutationError::Core(_) => Self::Domain(SandboxStoreError::Corrupt(
                "shared child reservation evidence is invalid".into(),
            )),
        }
    }
}

pub(super) async fn workspaces(
    store: &MongoSandboxStore,
    sandbox_id: &SandboxId,
    session: &mut ClientSession,
) -> Result<u64, CapacityError> {
    let mut ids = BTreeSet::new();
    let mut cursor = store
        .workspaces
        .find(doc! {"sandbox_id":sandbox_id.as_str(),"removed_at":Bson::Null})
        .limit(17)
        .session(&mut *session)
        .await?;
    while cursor.advance(&mut *session).await? {
        ids.insert(super::workspaces::read_workspace(&cursor.deserialize_current()?)?.id);
    }
    // The hard profile maximum is 16; the 17th row already proves refusal.
    if ids.len() > 16 {
        return Ok(ids.len() as u64);
    }
    let database = store.client.database(&store.sandboxes.namespace().db);
    for (record, _, released) in
        MongoBridgeAllocationStore::retained_reuse_reservations(&database, sandbox_id, session)
            .await?
    {
        if let AllocationPlacement::ReuseParent { target, parent, .. } = record.placement {
            if parent.workspace_created_at.is_none() && !released {
                ids.insert(target.workspace_id);
            }
        }
    }
    Ok(ids.len() as u64)
}

pub(super) async fn agents(
    store: &MongoSandboxStore,
    sandbox_id: &SandboxId,
    workspace_id: &WorkspaceId,
    session: &mut ClientSession,
) -> Result<u64, CapacityError> {
    let mut ids = BTreeSet::new();
    let mut cursor = store
        .agents
        .find(doc! {
            "workspace_id":workspace_id.as_str(),"status":{"$in":super::agents::LIVE_STATUSES},
            "record.deleted_at":{"$exists":false},
        })
        .limit(17)
        .session(&mut *session)
        .await?;
    while cursor.advance(&mut *session).await? {
        ids.insert(super::agents::read_agent(&cursor.deserialize_current()?)?.id);
    }
    if ids.len() > 16 {
        return Ok(ids.len() as u64);
    }
    let database = store.client.database(&store.sandboxes.namespace().db);
    for (record, released, _) in
        MongoBridgeAllocationStore::retained_reuse_reservations(&database, sandbox_id, session)
            .await?
    {
        let target = record.placement.target();
        if target.workspace_id == *workspace_id && !released {
            ids.insert(target.agent_id.clone());
        }
    }
    Ok(ids.len() as u64)
}
