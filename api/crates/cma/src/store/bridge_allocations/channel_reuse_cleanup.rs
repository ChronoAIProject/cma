//! Stage A request transaction. This does not schedule an Agent stop/removal,
//! call a provider, remove history or release reserved placement capacity.
use super::*;
use crate::{
    bridge::provisioning::reuse_cleanup::ChannelReuseCleanupAuthority,
    sandbox::{channel_reuse::state::ChannelReuseSnapshot, channel_reuse_cleanup::*},
    store::sandbox_mongo::{read_record, record_bson, DOCUMENT_SCHEMA_VERSION},
};
use mongodb::Database;
use std::collections::BTreeSet;

mod authority;
mod drain;
mod effects;
mod inventory;

const REQUESTS: &str = "channel_reuse_cleanup_requests";
const AUDITS: &str = "channel_reuse_cleanup_audits";

impl MongoBridgeAllocationStore {
    /// Caller supplies an explicit idempotent cleanup request after obtaining
    /// fresh original-human management. Merely resetting a partition never
    /// invokes this method. This receipt alone cannot authorize later effects.
    pub(crate) async fn request_channel_reuse_cleanup(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        request: &ChannelReuseCleanupRequest,
    ) -> Result<ChannelReuseCleanupState, AllocationError> {
        request.require(permit.allocation())?;
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(1 << (attempt - 1).min(5)))
                    .await;
            }
            permit.require_current()?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result = match self
                .cleanup_request_transaction(permit, request, &mut session)
                .await
            {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Core(error) => return Err(error),
                        MutationError::Mongo(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue
                        }
                        MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                // Retry the identical explicit request to recover the durable
                // receipt. Ambiguity grants no cleanup effect or new identity.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable)
                }
            }
        }
        Err(AllocationError::Unavailable)
    }

    async fn cleanup_request_transaction(
        &self,
        permit: &ChannelReuseCleanupAuthority,
        request: &ChannelReuseCleanupRequest,
        session: &mut ClientSession,
    ) -> Result<ChannelReuseCleanupState, MutationError> {
        let allocation_row = authority::fence(self, permit, session).await?;
        let allocation = permit.allocation();
        let database = self.bridges.client.database(&self.records.namespace().db);
        let requests: Collection<Document> = majority_collection(&database, REQUESTS);
        let id = ChannelReuseCleanupState::id(&allocation.id);
        if let Some(previous) = requests
            .find_one(doc! {"_id":&id})
            .session(&mut *session)
            .await?
        {
            let previous = read_state(&previous)?;
            if &previous.request != request
                || previous.requested_by != *permit.management().actor()
                || previous.seal.allocation_sha256 != digest(allocation)?
                || previous.seal.target != *allocation.placement.target()
                || previous.seal.owner != allocation.owner
            {
                return Err(AllocationError::Conflict.into());
            }
            let expected = doc! {"id":&previous.id,"digest":&previous.digest};
            if allocation_row
                .get_document("channel_reuse_cleanup_request")
                .ok()
                != Some(&expected)
            {
                return Err(AllocationError::Corrupt.into());
            }
            let audit = read(&database, AUDITS, &id, session).await?;
            if read_state(&audit)? != previous {
                return Err(AllocationError::Corrupt.into());
            }
            fence_row(&database, "bridge_allocations", allocation_row, session).await?;
            permit.require_current()?;
            return Ok(previous);
        }
        if allocation_row.contains_key("channel_reuse_cleanup_request") {
            return Err(AllocationError::Corrupt.into());
        }
        let (snapshot, mut holds) =
            inventory::read_owned(self, permit, &allocation_row, session).await?;
        let seal = ChannelReuseCleanupSeal::from_snapshot(&snapshot)?;
        holds.extend(effects::holds(&database, allocation, session).await?);
        if agent_busy(&snapshot) {
            holds.insert(ChannelReuseCleanupHold::AgentBusy);
        }
        if snapshot.workspace.as_ref().is_none_or(|workspace| {
            workspace.work.step != layer2_core::sandbox::WorkspaceWorkStep::None
                || workspace.work.lease.is_some()
                || workspace.work.execution.is_some()
        }) {
            holds.insert(ChannelReuseCleanupHold::WorkspaceBusy);
        }
        let state = ChannelReuseCleanupState::new(
            request.clone(),
            permit.management().actor().clone(),
            seal,
            holds.into_iter().collect(),
            mongodb::bson::DateTime::now().timestamp_millis(),
        )?;
        let row = state_document(&state)?;
        requests
            .insert_one(row.clone())
            .session(&mut *session)
            .await?;
        majority_collection::<Document>(&database, AUDITS)
            .insert_one(row)
            .session(&mut *session)
            .await?;
        let changed = self.records.update_one(allocation_row,
            doc! {"$set":{"channel_reuse_cleanup_request":{"id":&state.id,"digest":&state.digest}},
                "$inc":{"channel_reuse_cleanup_fence":1_i64}}).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
        permit.require_current()?;
        Ok(state)
    }
}

fn digest(value: &impl serde::Serialize) -> Result<String, AllocationError> {
    layer2_core::profiles::content_digest(value).map_err(|_| AllocationError::Corrupt)
}
fn state_document(state: &ChannelReuseCleanupState) -> Result<Document, AllocationError> {
    state.validate()?;
    Ok(
        doc! {"_id":&state.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "allocation_id":&state.seal.allocation_id,"sandbox_id":state.seal.target.sandbox_id.as_str(),
        "record":record_bson(state).map_err(|_| AllocationError::Corrupt)?},
    )
}
fn read_state(row: &Document) -> Result<ChannelReuseCleanupState, AllocationError> {
    let state: ChannelReuseCleanupState = read_record(row).map_err(|_| AllocationError::Corrupt)?;
    let expected = state_document(&state)?;
    if expected
        .iter()
        .any(|(key, value)| row.get(key) != Some(value))
    {
        return Err(AllocationError::Corrupt);
    }
    Ok(state)
}
async fn read(
    database: &Database,
    name: &str,
    id: &str,
    session: &mut ClientSession,
) -> Result<Document, MutationError> {
    majority_collection::<Document>(database, name)
        .find_one(doc! {"_id":id})
        .session(session)
        .await?
        .ok_or(AllocationError::Corrupt.into())
}
async fn fence_row(
    database: &Database,
    name: &str,
    row: Document,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let changed = majority_collection::<Document>(database, name)
        .update_one(row, doc! {"$inc":{"channel_reuse_cleanup_fence":1_i64}})
        .session(session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(())
}
