use super::*;
use crate::store::{
    bridge_credentials::control_plane::ControlPlaneFenceError,
    bridge_reuse_parent::ParentError,
    resource_access::{fence_lineage, read_execution_snapshot, ReadError},
};
use layer2_core::bridge::allocation::AllocationState;

pub(super) async fn fence(
    store: &MongoBridgeAllocationStore,
    permit: &ChannelReuseCleanupAuthority,
    session: &mut ClientSession,
) -> Result<Document, MutationError> {
    let row = fence_common(store, permit, session).await?;
    store
        .fence_cleanup_execution_job(permit, &row, session)
        .await
}

pub(super) async fn fence_common(
    store: &MongoBridgeAllocationStore,
    permit: &ChannelReuseCleanupAuthority,
    session: &mut ClientSession,
) -> Result<Document, MutationError> {
    permit.require_current()?;
    let allocation = permit.allocation();
    let bridge = permit.management().record();
    let current = store
        .bridges
        .definitions
        .find_one(doc! {"_id":bridge.id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    if crate::store::bridge_definitions::decode_bridge(&current)
        .map_err(|_| AllocationError::Corrupt)?
        != *bridge
    {
        return Err(AllocationError::Conflict.into());
    }
    store
        .bridges
        .credentials
        .require_control_plane_binding(bridge, permit.human(), session)
        .await
        .map_err(|error| match error {
            ControlPlaneFenceError::Core(_) => {
                MutationError::Core(AllocationError::AuthorityUnavailable)
            }
            ControlPlaneFenceError::Driver(error) => MutationError::Mongo(error),
        })?;
    let resources = &store
        .invokes
        .as_ref()
        .ok_or(AllocationError::AuthorityUnavailable)?
        .resources;
    let database_name = store.records.namespace().db;
    if database_name != resources.sandboxes.namespace().db
        || database_name != store.bridges.definitions.namespace().db
        || database_name != store.heads.namespace().db
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let target = allocation
        .config
        .placement
        .reused_parent()
        .ok_or(AllocationError::IdentityMismatch)?;
    let current = read_execution_snapshot(resources, &target, session)
        .await
        .map_err(read_error)?;
    if &current != permit.parent() {
        return Err(AllocationError::Conflict.into());
    }
    resources
        .ordinary_reuse_parent(&current.lineage.sandbox, session)
        .await
        .map_err(|error| match error {
            ParentError::Core(error) => MutationError::Core(error),
            ParentError::Driver(error) => MutationError::Mongo(error),
        })?;
    fence_lineage(resources, &current, session)
        .await
        .map_err(read_error)?;
    if let Some(policy) = &current.policy {
        let changed = resources
            .resource_policies
            .update_one(
                doc! {"_id":current.lineage.sandbox.id.as_str(),"revision":policy.revision as i64,
                "document_schema_version":DOCUMENT_SCHEMA_VERSION},
                doc! {"$inc":{"channel_reuse_cleanup_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
    }
    // Share changes and absent-policy insertion also write the same parent
    // fence. Complete management snapshot equality is checked above.
    let row = store
        .records
        .find_one(doc! {"_id":&allocation.id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    if records::decode_record(&row)? != *allocation {
        return Err(AllocationError::Conflict.into());
    }
    fence_head(store, allocation, session).await?;
    permit.require_current()?;
    Ok(row)
}

async fn fence_head(
    store: &MongoBridgeAllocationStore,
    original: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let head = store
        .heads
        .find_one(doc! {"_id":original.identity.head_id()?})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    let head_id = records::decode_head(&head, &original.identity)?;
    let generation = u64::try_from(
        head.get_i64("generation")
            .map_err(|_| AllocationError::Corrupt)?,
    )
    .map_err(|_| AllocationError::Corrupt)?;
    if generation <= original.identity.generation || head_id == original.id {
        return Err(AllocationError::Conflict.into());
    }
    // Canonical identities bound this complete walk to 256 generations. Every
    // edge must be the exact reset transition, including its retirement time.
    let mut previous = original.clone();
    for next_generation in original.identity.generation + 1..=generation {
        let mut identity = original.identity.clone();
        identity.generation = next_generation;
        let next_id = identity.id()?;
        let row = store
            .records
            .find_one(doc! {"_id":&next_id})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::IdentityMismatch)?;
        let next = records::decode_record(&row)?;
        if next.identity != identity
            || next.owner != original.owner
            || next.predecessor.as_ref().is_none_or(|predecessor| {
                predecessor.id != previous.id
                    || predecessor.generation != previous.identity.generation
            })
            || previous.state != AllocationState::Retired
            || previous.lease.is_some()
            || previous.updated_at_ms != next.created_at_ms
        {
            return Err(AllocationError::IdentityMismatch.into());
        }
        // Reset or mutation of any intermediate row must conflict with this
        // transaction. Fencing the head alone does not retain the full proof.
        let changed = store
            .records
            .update_one(row, doc! {"$inc":{"channel_reuse_cleanup_fence":1_i64}})
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
        previous = next;
    }
    if previous.id != head_id || previous.state == AllocationState::Retired {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let changed = store
        .heads
        .update_one(head, doc! {"$inc":{"channel_reuse_cleanup_fence":1_i64}})
        .session(session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(())
}

fn read_error(error: ReadError) -> MutationError {
    match error {
        ReadError::Driver(error) => MutationError::Mongo(error),
        ReadError::Domain(_) => MutationError::Core(AllocationError::Conflict),
    }
}
