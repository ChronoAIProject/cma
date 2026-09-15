//! Exact reset lineage for an already accepted command or explicit reply.
use layer2_core::bridge::allocation::{AllocationRecord, AllocationState};
use layer2_core::bridge::invoke::{BridgeInvokeError, ChannelInvokeBasis};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

use super::{MongoBridgeInvokeStore, MutationError};
use crate::store::{
    bridge_allocations::{decode_head, decode_record},
    durability::majority_collection,
};

impl MongoBridgeInvokeStore {
    pub(super) async fn require_retained_channel_grant(
        &self,
        basis: &ChannelInvokeBasis,
        observed: &AllocationRecord,
        write_fence: bool,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let current = self.channel_snapshot(&basis.allocation.id, session).await?;
        if &current.basis.allocation != observed || current.basis.creation != basis.creation {
            return Err(BridgeInvokeError::TargetMismatch.into());
        }
        basis.require_retained_allocation(observed)?;
        if observed.state == AllocationState::Retired {
            self.require_reset_lineage(observed, write_fence, session)
                .await?;
        }
        if write_fence {
            for (collection, filter) in [
                (
                    self.channel_allocations(),
                    doc! {"_id":&observed.id,"revision":observed.revision as i64},
                ),
                (
                    self.resources.agent_creation_provenance.clone(),
                    doc! {"_id":basis.creation.agent_id.as_str()},
                ),
            ] {
                let changed = collection
                    .update_one(filter, doc! {"$inc":{"retained_command_fence":1_i64}})
                    .session(&mut *session)
                    .await?;
                if changed.matched_count != 1 {
                    return Err(BridgeInvokeError::Conflict.into());
                }
            }
        }
        Ok(())
    }

    async fn require_reset_lineage(
        &self,
        original: &AllocationRecord,
        write_fence: bool,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        let database = self.bridges.client.database(&self.grants.namespace().db);
        let heads = majority_collection::<Document>(&database, "bridge_allocation_heads");
        let head = heads
            .find_one(
                doc! {"_id":original.identity.head_id().map_err(|_| BridgeInvokeError::Corrupt)?},
            )
            .session(&mut *session)
            .await?
            .ok_or(BridgeInvokeError::TargetMismatch)?;
        let head_id =
            decode_head(&head, &original.identity).map_err(|_| BridgeInvokeError::Corrupt)?;
        let generation = head
            .get_i64("generation")
            .map_err(|_| BridgeInvokeError::Corrupt)?;
        let generation = u64::try_from(generation).map_err(|_| BridgeInvokeError::Corrupt)?;
        // AllocationIdentity::id/decode_head bounds this complete walk to 256 records.
        if generation <= original.identity.generation || head_id == original.id {
            return Err(BridgeInvokeError::TargetMismatch.into());
        }
        let allocations = self.channel_allocations();
        let mut previous = original.clone();
        for next_generation in original.identity.generation + 1..=generation {
            let mut identity = original.identity.clone();
            identity.generation = next_generation;
            let next_id = identity.id().map_err(|_| BridgeInvokeError::Corrupt)?;
            let row = allocations
                .find_one(doc! {"_id":&next_id})
                .session(&mut *session)
                .await?
                .ok_or(BridgeInvokeError::TargetMismatch)?;
            let next = decode_record(&row).map_err(|_| BridgeInvokeError::Corrupt)?;
            if next.identity != identity
                || next.owner != original.owner
                || next.predecessor.as_ref().is_none_or(|p| {
                    p.id != previous.id || p.generation != previous.identity.generation
                })
                || previous.state != AllocationState::Retired
                || previous.lease.is_some()
                || previous.updated_at_ms != next.created_at_ms
            {
                return Err(BridgeInvokeError::TargetMismatch.into());
            }
            if write_fence {
                // Every row on which the proof depends conflicts with reset,
                // deletion or a concurrent mutation of its exact predecessor.
                let changed = allocations
                    .update_one(row, doc! {"$inc":{"retained_command_fence":1_i64}})
                    .session(&mut *session)
                    .await?;
                if changed.matched_count != 1 {
                    return Err(BridgeInvokeError::Conflict.into());
                }
            }
            previous = next;
        }
        if previous.id != head_id || previous.state == AllocationState::Retired {
            return Err(BridgeInvokeError::TargetMismatch.into());
        }
        if write_fence {
            let changed = heads
                .update_one(head, doc! {"$inc":{"retained_command_fence":1_i64}})
                .session(session)
                .await?;
            if changed.matched_count != 1 {
                return Err(BridgeInvokeError::Conflict.into());
            }
        }
        Ok(())
    }
}
