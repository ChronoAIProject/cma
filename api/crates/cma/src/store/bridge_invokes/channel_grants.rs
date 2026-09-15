//! Exact channel creation reads and grant fences share the resource transaction.
use layer2_core::bridge::allocation::AllocationRecord;
use layer2_core::bridge::invoke::{BridgeInvokeError, ChannelInvokeBasis, ChannelInvokeSnapshot};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};

use super::{MongoBridgeInvokeStore, MutationError};
use crate::store::{
    durability::majority_collection,
    sandbox_mongo::{is_transient, start_transaction, TRANSACTION_ATTEMPTS},
};

impl MongoBridgeInvokeStore {
    fn channel_allocations(&self) -> Collection<Document> {
        majority_collection(
            &self.bridges.client.database(&self.grants.namespace().db),
            "bridge_allocations",
        )
    }

    pub(super) async fn read_channel_target(
        &self,
        allocation_id: &str,
    ) -> Result<ChannelInvokeSnapshot, BridgeInvokeError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| BridgeInvokeError::Unavailable)?;
            let result = self.channel_snapshot(allocation_id, &mut session).await;
            let _ = session.abort_transaction().await;
            match result {
                Ok(snapshot) => return Ok(snapshot),
                Err(MutationError::Mongo(error)) if is_transient(&error) => continue,
                Err(error) => return Err(error.into_invoke()),
            }
        }
        Err(BridgeInvokeError::Unavailable)
    }

    async fn channel_snapshot(
        &self,
        allocation_id: &str,
        session: &mut ClientSession,
    ) -> Result<ChannelInvokeSnapshot, MutationError> {
        let document = self
            .channel_allocations()
            .find_one(doc! {"_id":allocation_id})
            .session(&mut *session)
            .await?
            .ok_or(BridgeInvokeError::NotFound)?;
        let allocation: AllocationRecord =
            crate::store::bridge_allocations::decode_record(&document)
                .map_err(|_| BridgeInvokeError::Corrupt)?;
        let provenance = crate::store::agent_provenance::read_snapshot(
            &self.resources,
            &allocation.placement.target().agent_id,
            session,
        )
        .await?;
        let creation = provenance
            .creation
            .ok_or(BridgeInvokeError::TargetMismatch)?;
        let basis = ChannelInvokeBasis {
            allocation,
            creation,
        };
        basis.require_resources(&provenance.resource)?;
        Ok(ChannelInvokeSnapshot {
            basis,
            resources: provenance.resource,
        })
    }

    pub(super) async fn fence_channel_grant(
        &self,
        basis: &ChannelInvokeBasis,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let current = self.channel_snapshot(&basis.allocation.id, session).await?;
        if &current.basis != basis {
            return Err(BridgeInvokeError::TargetMismatch.into());
        }
        // Real writes conflict with revocation/reset/corruption of both exact
        // retained documents; historical sidecars cannot authorize substitutes.
        let changed = self
            .channel_allocations()
            .update_one(
                doc! {"_id":&basis.allocation.id, "revision":basis.allocation.revision as i64},
                doc! {"$inc":{"invoke_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        let changed = self
            .resources
            .agent_creation_provenance
            .update_one(
                doc! {"_id":basis.creation.agent_id.as_str()},
                doc! {"$inc":{"invoke_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(BridgeInvokeError::Conflict.into());
        }
        Ok(())
    }
}
