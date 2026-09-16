use super::*;
use crate::store::{
    durability::majority_collection, resource_access::read_inventory_policy,
    sandboxes::read_sandbox,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection, Database,
};

pub(super) async fn read_fenced(
    store: &MongoBridgeAllocationStore,
    permit: &ChannelRetirementAuthority,
    session: &mut ClientSession,
) -> Result<(ChannelRetirementSnapshot, Document), MutationError> {
    permit.require_current()?;
    let authority = permit.allocation_authority();
    let lease = permit.human();
    let allocation = permit.allocation();
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
    // Reset retains the original completed allocation's ownership of this
    // exact root; the current head still fences its retained configuration.
    let retained = match allocation.state {
        AllocationState::Ready => head_generation == original_generation,
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
    let tree = super::lineage::require(&database, allocation, &sandbox, session).await?;
    let snapshot = ChannelRetirementSnapshot {
        allocation: allocation.clone(),
        sandbox,
        creation: decode(&document, "channel_creation")?,
        provisioning: decode(&document, "channel_provisioning")?,
        lifecycle: optional(&document, "channel_lifecycle_credential")?,
        renewal: optional(&document, "channel_renewal")?,
        state: optional(&document, "channel_retirement")?,
        retention: optional(&document, "channel_storage_retention")?,
        tree,
    };
    snapshot.validate()?;
    if let Some(retention) = &snapshot.retention {
        retention.require_human(permit.human())?;
        crate::store::checkpoint_objects::verify_channel_retention(&database, retention, session)
            .await?;
    }
    if &snapshot.sandbox != permit.sandbox() {
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
    fence(
        &majority_collection(&database, "sandboxes"),
        doc! {"_id":snapshot.sandbox.id.as_str(),"revision":snapshot.sandbox.revision as i64},
        session,
    )
    .await?;
    Ok((snapshot, document))
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

fn optional<T: serde::de::DeserializeOwned>(
    document: &Document,
    key: &str,
) -> Result<Option<T>, AllocationError> {
    document
        .get(key)
        .map(|value| {
            mongodb::bson::deserialize_from_bson(value.clone())
                .map_err(|_| AllocationError::Corrupt)
        })
        .transpose()
}
