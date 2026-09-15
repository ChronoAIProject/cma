use super::*;
use layer2_core::bridge::allocation::AllocationState;

pub(super) async fn fence(
    store: &MongoBridgeAllocationStore,
    allocation: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let head_id = allocation.identity.head_id()?;
    let row = store
        .heads
        .find_one(doc! {"_id":&head_id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    let current_id = records::decode_head(&row, &allocation.identity)?;
    let generation = row
        .get_i64("generation")
        .map_err(|_| AllocationError::Corrupt)?;
    let original =
        i64::try_from(allocation.identity.generation).map_err(|_| AllocationError::Corrupt)?;
    let valid = match allocation.state {
        AllocationState::Ready => generation == original && current_id == allocation.id,
        AllocationState::Retired => generation > original && current_id != allocation.id,
        _ => false,
    };
    if !valid {
        return Err(AllocationError::Conflict.into());
    }
    if current_id != allocation.id {
        let next = store
            .records
            .find_one(doc! {"_id":&current_id})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::Corrupt)?;
        let next = records::decode_record(&next)?;
        if next.owner != allocation.owner
            || next.identity.bridge_id != allocation.identity.bridge_id
            || next.identity.key != allocation.identity.key
            || i64::try_from(next.identity.generation).map_err(|_| AllocationError::Corrupt)?
                != generation
        {
            return Err(AllocationError::IdentityMismatch.into());
        }
    }
    // Accepted DrainOriginal work keeps the old target after reset. The current
    // Bridge/grant proof separately refuses SuspendOriginal or revoked work.
    let changed = store
        .heads
        .update_one(row, doc! {"$inc":{"channel_reuse_revival_fence":1_i64}})
        .session(session)
        .await?;
    if changed.matched_count != 1 || changed.modified_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(())
}
