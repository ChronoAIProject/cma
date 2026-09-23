//! Ordinary admission accounts for accepted Channel children before they exist.
//! Callers hold the existing parent transaction fence and handle replay first.
use std::collections::BTreeSet;

use layer2_core::{sandbox::store::SandboxStoreError, SandboxId, WorkspaceId};
use mongodb::{
    bson::{doc, Bson},
    ClientSession,
};

use super::sandbox_mongo::MongoSandboxStore;

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
    // Old accepted child reservations remain capacity authority until an explicit migration.
    // Presence (including malformed records) must not make capacity appear available.
    let retained = database.collection::<mongodb::bson::Document>("bridge_allocations")
        .find_one(doc! {"record.placement.target.sandbox_id": sandbox_id.as_str(), "record.placement.kind":"reuse_parent"})
        .projection(doc! {"_id":1}).session(&mut *session).await?;
    if retained.is_some() {
        return Err(SandboxStoreError::Unavailable(
            "retained legacy child reservations require migration".into(),
        )
        .into());
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
    // Old accepted child reservations remain capacity authority until an explicit migration.
    // Presence (including malformed records) must not make capacity appear available.
    let retained = database.collection::<mongodb::bson::Document>("bridge_allocations")
        .find_one(doc! {"record.placement.target.sandbox_id": sandbox_id.as_str(), "record.placement.kind":"reuse_parent"})
        .projection(doc! {"_id":1}).session(&mut *session).await?;
    if retained.is_some() {
        return Err(SandboxStoreError::Unavailable(
            "retained legacy child reservations require migration".into(),
        )
        .into());
    }
    Ok(ids.len() as u64)
}
