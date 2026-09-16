mod audit;
mod claim;
mod save;
use super::*;
use crate::store::{
    durability::majority_collection, resource_access::read_inventory_policy,
    sandboxes::read_sandbox,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection, Database,
};

pub(super) async fn apply(
    store: &MongoBridgeAllocationStore,
    authority: &AllocationAuthority,
    lease: &BridgeControlPlaneLease,
    allocation: &AllocationRecord,
    change: &Change<'_>,
    session: &mut ClientSession,
) -> Result<RenewalAdmission, MutationError> {
    store.fence(authority, session).await?;
    store
        .bridges
        .credentials
        .require_control_plane_binding(authority.bridge(), lease, session)
        .await
        .map_err(|error| match error {
            crate::store::bridge_credentials::control_plane::ControlPlaneFenceError::Core(_) => {
                MutationError::Core(AllocationError::AuthorityUnavailable)
            }
            crate::store::bridge_credentials::control_plane::ControlPlaneFenceError::Driver(
                error,
            ) => MutationError::Mongo(error),
        })?;
    let current = store
        .records
        .find_one(doc! {"_id":&allocation.id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    if &super::super::records::decode_record(&current)? != allocation {
        return Err(AllocationError::Conflict.into());
    }
    let head_id = allocation.identity.head_id()?;
    let head = store
        .heads
        .find_one(doc! {"_id":&head_id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    let current_head = super::super::records::decode_head(&head, &allocation.identity)?;
    let head_generation = head
        .get_i64("generation")
        .map_err(|_| AllocationError::Corrupt)?;
    let original_generation =
        i64::try_from(allocation.identity.generation).map_err(|_| AllocationError::Corrupt)?;
    // Reset retires only the allocation record and retains its Ready receipt;
    // the original Active Sandbox can still serve accepted work under drain.
    let retained = match allocation.state {
        AllocationState::Ready => head_generation == original_generation,
        AllocationState::Retired
            if head_generation == original_generation && current_head == allocation.id =>
        {
            store.require_detached_head(allocation, session).await?;
            true
        }
        AllocationState::Retired => head_generation > original_generation,
        _ => false,
    };
    if !retained {
        return Err(AllocationError::Conflict.into());
    }
    if current_head != allocation.id {
        let head_document = store
            .records
            .find_one(doc! {"_id":&current_head})
            .session(&mut *session)
            .await?
            .ok_or(AllocationError::Corrupt)?;
        let target = super::super::records::decode_record(&head_document)?;
        if target.owner != allocation.owner
            || target.identity.bridge_id != allocation.identity.bridge_id
            || target.identity.key != allocation.identity.key
            || i64::try_from(target.identity.generation).map_err(|_| AllocationError::Corrupt)?
                != head_generation
        {
            return Err(AllocationError::IdentityMismatch.into());
        }
    }
    let namespace = store.records.namespace();
    if namespace.db != store.bridges.definitions.namespace().db
        || namespace.db != store.heads.namespace().db
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let database = store.bridges.client.database(&namespace.db);
    let document = read(
        &database,
        "sandboxes",
        allocation.placement.target().sandbox_id.as_str(),
        session,
    )
    .await?;
    let sandbox = read_sandbox(&document).map_err(|_| AllocationError::Corrupt)?;
    if current.get("sandbox_admission") != document.get("channel_creation") {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let snapshot = ChannelRenewalSnapshot {
        allocation: allocation.clone(),
        sandbox,
        creation: decode(&document, "channel_creation")?,
        provisioning: decode(&document, "channel_provisioning")?,
        lifecycle_credential: document
            .get("channel_lifecycle_credential")
            .map(|value| {
                mongodb::bson::deserialize_from_bson(value.clone())
                    .map_err(|_| AllocationError::Corrupt)
            })
            .transpose()?,
        renewal: document
            .get("channel_renewal")
            .map(|value| {
                mongodb::bson::deserialize_from_bson(value.clone())
                    .map_err(|_| AllocationError::Corrupt)
            })
            .transpose()?,
    };
    snapshot.validate()?;
    if change
        .previous()
        .is_some_and(|previous| previous != &snapshot)
    {
        return Err(AllocationError::Conflict.into());
    }
    let policy = read(
        &database,
        "resource_policies",
        snapshot.sandbox.id.as_str(),
        session,
    )
    .await?;
    let policy = read_inventory_policy(&policy).map_err(|_| AllocationError::Corrupt)?;
    if policy.owner != allocation.owner || policy.sandbox_created_at != snapshot.sandbox.created_at
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    fence(
        &store.heads,
        // Reset does not cancel accepted work on an older Ready allocation.
        // authority.require_record + store.fence above enforce the original
        // retained config and current DrainOriginal/SuspendOriginal policy.
        doc! {"_id":&head_id,"allocation_id":&current_head},
        session,
    )
    .await?;
    fence(
        &store.records,
        doc! {"_id":&allocation.id,"revision":allocation.revision as i64},
        session,
    )
    .await?;
    fence(
        &majority_collection(&database, "resource_policies"),
        doc! {"_id":snapshot.sandbox.id.as_str(),"revision":policy.revision as i64},
        session,
    )
    .await?;
    save::apply(&database, snapshot, &document, change, lease, session).await
}

async fn read(
    database: &Database,
    name: &str,
    id: &str,
    session: &mut ClientSession,
) -> Result<Document, MutationError> {
    let rows: Collection<Document> = majority_collection(database, name);
    rows.find_one(doc! {"_id":id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Corrupt.into())
}
fn decode<T: serde::de::DeserializeOwned>(
    document: &Document,
    key: &str,
) -> Result<T, AllocationError> {
    mongodb::bson::deserialize_from_bson(
        document.get(key).cloned().ok_or(AllocationError::Corrupt)?,
    )
    .map_err(|_| AllocationError::Corrupt)
}
async fn fence(
    rows: &Collection<Document>,
    filter: Document,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let changed = rows
        .update_one(filter, doc! {"$inc":{"channel_execution_fence":1_i64}})
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(())
}
