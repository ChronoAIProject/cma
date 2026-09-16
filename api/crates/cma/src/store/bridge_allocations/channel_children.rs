//! Child resources and Channel stage progress share the existing exact grant,
//! allocation head, owner policy and Sandbox transaction fences.
use layer2_core::bridge::allocation::{AllocationAuthority, AllocationError};
use layer2_core::sandbox::Workspace;
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};

use super::{MongoBridgeAllocationStore, MutationError};
use crate::bridge::BridgeControlPlaneLease;
use crate::sandbox::{
    channel_children::ChannelChildEvidence,
    channel_provisioning::state::ChannelProvisioningSnapshot,
};
use crate::store::{
    durability::majority_collection,
    sandbox_mongo::{commit_transaction, is_transient, start_transaction, CommitOutcome},
};

pub(super) mod transaction;

impl MongoBridgeAllocationStore {
    pub(crate) async fn channel_workspace(
        &self,
        snapshot: &ChannelProvisioningSnapshot,
    ) -> Result<Workspace, AllocationError> {
        let database = self.bridges.client.database(&self.records.namespace().db);
        let collection: Collection<Document> = majority_collection(&database, "workspaces");
        let document = collection
            .find_one(doc! {"_id":snapshot.allocation.placement.target().workspace_id.as_str()})
            .await
            .map_err(|_| AllocationError::Unavailable)?
            .ok_or(AllocationError::Corrupt)?;
        transaction::read_workspace(snapshot, &document)
    }

    pub(crate) async fn commit_channel_child(
        &self,
        authority: &AllocationAuthority,
        lease: &BridgeControlPlaneLease,
        snapshot: &ChannelProvisioningSnapshot,
        evidence: &ChannelChildEvidence,
    ) -> Result<ChannelProvisioningSnapshot, AllocationError> {
        for attempt in 0..16 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(
                    1_u64 << (attempt - 1).min(5),
                ))
                .await;
            }
            authority.require_record(&snapshot.allocation)?;
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| AllocationError::Unavailable)?;
            let result =
                match transaction::apply(self, authority, lease, snapshot, evidence, &mut session)
                    .await
                {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = session.abort_transaction().await;
                        match error {
                            MutationError::Core(error) => return Err(error),
                            MutationError::Mongo(error) if is_transient(&error) => continue,
                            MutationError::Mongo(_) => return Err(AllocationError::Unavailable),
                        }
                    }
                };
            if authority.require_current().is_err()
                || tokio::time::Instant::now() >= lease.deadline()
                || snapshot.allocation.lease.as_ref().is_none_or(|claim| {
                    claim.expires_at_ms <= mongodb::bson::DateTime::now().timestamp_millis()
                })
            {
                let _ = session.abort_transaction().await;
                return Err(AllocationError::AuthorityUnavailable);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(AllocationError::Unavailable)
                }
            }
        }
        Err(AllocationError::Unavailable)
    }
}
