mod load;
mod save;

use super::{
    super::{MongoBridgeAllocationStore, MutationError},
    StageChange,
};
use crate::bridge::BridgeControlPlaneLease;
use crate::sandbox::channel_provisioning::state::{
    ChannelProvisioningSettings, ChannelProvisioningSnapshot,
};
use crate::store::durability::majority_collection;
use layer2_core::bridge::allocation::{
    AllocationAuthority, AllocationError, AllocationRecord, AllocationState,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};

struct Resources {
    database: String,
    sandboxes: Collection<Document>,
    policies: Collection<Document>,
    events: Collection<Document>,
}

pub(in crate::store::bridge_allocations) async fn apply(
    store: &MongoBridgeAllocationStore,
    authority: &AllocationAuthority,
    lease: &BridgeControlPlaneLease,
    allocation: &AllocationRecord,
    settings: &ChannelProvisioningSettings,
    change: Option<&StageChange<'_>>,
    session: &mut ClientSession,
) -> Result<ChannelProvisioningSnapshot, MutationError> {
    require_lease(allocation, lease)?;
    settings.validate()?;
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
    if super::super::records::decode_head(&head, &allocation.identity)? != allocation.id {
        return Err(AllocationError::Conflict.into());
    }
    let changed = store
        .heads
        .update_one(
            doc! {"_id":&head_id,"allocation_id":&allocation.id},
            doc! {"$inc":{"channel_execution_fence":1_i64}},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    let namespace = store.records.namespace();
    if namespace.db != store.bridges.definitions.namespace().db
        || namespace.db != store.heads.namespace().db
    {
        return Err(AllocationError::IdentityMismatch.into());
    }
    let database = store.bridges.client.database(&namespace.db);
    let resources = Resources {
        database: namespace.db,
        sandboxes: majority_collection(&database, "sandboxes"),
        policies: majority_collection(&database, "resource_policies"),
        events: majority_collection(&database, "sandbox_events"),
    };
    let (snapshot, document, policy) =
        load::snapshot(&resources, allocation, &current, settings, session).await?;
    let snapshot = save::apply(store, &resources, snapshot, &document, change, session).await?;
    let changed = resources
        .policies
        .update_one(
            doc! {"_id":snapshot.sandbox.id.as_str(),"revision":policy.revision as i64},
            doc! {"$inc":{"channel_execution_fence":1_i64}},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    require_lease(allocation, lease)?;
    authority.require_record(&snapshot.allocation)?;
    snapshot.validate()?;
    Ok(snapshot)
}

fn require_lease(
    allocation: &AllocationRecord,
    lease: &BridgeControlPlaneLease,
) -> Result<(), AllocationError> {
    if tokio::time::Instant::now() >= lease.deadline()
        || allocation
            .lease
            .as_ref()
            .is_none_or(|claim| claim.expires_at_ms <= now_ms().unwrap_or(i64::MAX))
        || !allocation.effect_started
        || !matches!(
            allocation.state,
            AllocationState::NeedsConnection
                | AllocationState::Provisioning
                | AllocationState::FailedRetryable
        )
        || allocation.owner != *lease.owner()
        || allocation.identity.bridge_id != *lease.bridge_id()
        || allocation.config.connector != *lease.connector()
    {
        return Err(AllocationError::AuthorityUnavailable);
    }
    Ok(())
}

fn now_ms() -> Result<i64, AllocationError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|v| i64::try_from(v.as_millis()).ok())
        .ok_or(AllocationError::Unavailable)
}
