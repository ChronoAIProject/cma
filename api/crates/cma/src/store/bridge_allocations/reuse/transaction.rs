use super::*;
use crate::store::{sandbox_mongo::record_bson, MongoSandboxStore};
use layer2_core::{
    bridge::allocation::AllocationState,
    sandbox::{SandboxEventKind, SandboxStatus, WorkspaceStatus},
};
use mongodb::Database;

pub(super) fn require_current(
    authority: &AllocationAuthority,
    lease: &BridgeControlPlaneLease,
    allocation: &AllocationRecord,
) -> Result<(), AllocationError> {
    authority.require_record(allocation)?;
    if !matches!(authority, AllocationAuthority::Reuse(_))
        || tokio::time::Instant::now() >= lease.deadline()
        || allocation.lease.as_ref().is_none_or(|claim| {
            claim.expires_at_ms <= mongodb::bson::DateTime::now().timestamp_millis()
        })
        || !allocation.effect_started
        || !matches!(
            allocation.state,
            AllocationState::Provisioning
                | AllocationState::NeedsConnection
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

pub(super) async fn load(
    store: &MongoBridgeAllocationStore,
    authority: &AllocationAuthority,
    lease: &BridgeControlPlaneLease,
    allocation: &AllocationRecord,
    settings: &ChannelProvisioningSettings,
    session: &mut ClientSession,
) -> Result<ChannelReuseSnapshot, MutationError> {
    settings.validate()?;
    let document = fence_allocation(store, authority, lease, allocation, session).await?;
    let AllocationAuthority::Reuse(proof) = authority else {
        return Err(AllocationError::IdentityMismatch.into());
    };
    let sandbox = proof.resources().lineage.sandbox.clone();
    if sandbox.status != SandboxStatus::Active
        || sandbox.lifecycle.checkpoint_capture.is_some()
        || sandbox.lifecycle.operation.is_some()
        || !sandbox.credential.delivered
        || sandbox.runtime.is_none()
        || sandbox.work.lease.is_some()
        || sandbox.work.execution.is_some()
        || !matches!(
            sandbox.work.step,
            layer2_core::sandbox::SandboxWorkStep::None
                | layer2_core::sandbox::SandboxWorkStep::WaitForCredentialRotation
        )
    {
        return Err(AllocationError::Unavailable.into());
    }
    let state = if let Some(value) = document.get("channel_reuse") {
        let state: ChannelReuseState = mongodb::bson::deserialize_from_bson(value.clone())
            .map_err(|_| AllocationError::Corrupt)?;
        if &state.settings != settings {
            return Err(AllocationError::IdentityMismatch.into());
        }
        state
    } else {
        let state = ChannelReuseState::initial(allocation, settings.clone())?;
        let changed=store.records.update_one(doc! {"_id":&allocation.id,"revision":allocation.revision as i64,"channel_reuse":{"$exists":false}},
            doc! {"$set":{"channel_reuse":record_bson(&state).map_err(|_| AllocationError::Corrupt)?}}).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
        state
    };
    let database = store.bridges.client.database(&store.records.namespace().db);
    let target = allocation.placement.target();
    let workspace_row = read_optional(
        &database,
        "workspaces",
        target.workspace_id.as_str(),
        session,
    )
    .await?;
    let agent_row = read_optional(&database, "agents", target.agent_id.as_str(), session).await?;
    let tree_row =
        read_optional(&database, "worktrees", target.worktree_id.as_str(), session).await?;
    let creation_row = read_optional(
        &database,
        "agent_creation_provenance",
        target.agent_id.as_str(),
        session,
    )
    .await?;
    let snapshot = ChannelReuseSnapshot {
        allocation: allocation.clone(),
        sandbox,
        state,
        workspace: workspace_row
            .as_ref()
            .map(crate::store::workspaces::read_workspace)
            .transpose()
            .map_err(|_| AllocationError::Corrupt)?,
        worktree: tree_row
            .as_ref()
            .map(crate::store::worktrees::read_worktree)
            .transpose()
            .map_err(|_| AllocationError::Corrupt)?,
        agent: agent_row
            .as_ref()
            .map(crate::store::agents::read_agent)
            .transpose()
            .map_err(|_| AllocationError::Corrupt)?,
        creation: creation_row
            .as_ref()
            .map(crate::store::agent_provenance::read)
            .transpose()
            .map_err(|_| AllocationError::Corrupt)?,
        workspace_creation: workspace_row
            .as_ref()
            .and_then(|row| row.get("channel_reuse_creation"))
            .map(|value| {
                mongodb::bson::deserialize_from_bson(value.clone())
                    .map_err(|_| AllocationError::Corrupt)
            })
            .transpose()?,
    };
    snapshot.require(authority)?;
    if proof.parent().workspace_created_at.is_some()
        && snapshot.workspace.as_ref().is_none_or(|workspace| {
            workspace.status != WorkspaceStatus::Ready
                || workspace.work.step != layer2_core::sandbox::WorkspaceWorkStep::None
                || workspace.work.lease.is_some()
        })
    {
        return Err(AllocationError::Unavailable.into());
    }
    for (collection, row) in [
        ("workspaces", workspace_row),
        ("worktrees", tree_row),
        ("agents", agent_row),
        ("agent_creation_provenance", creation_row),
    ] {
        if let Some(row) = row {
            if collection == "workspaces"
                && (row.contains_key("owned_workspace_cleanup_admission_v1")
                    || row.contains_key("channel_reuse_cleanup_execution"))
            {
                return Err(AllocationError::Conflict.into());
            }

            let rows: Collection<Document> = majority_collection(&database, collection);
            let changed = rows
                .update_one(row, doc! {"$inc":{"channel_reuse_fence":1_i64}})
                .session(&mut *session)
                .await?;
            if changed.matched_count != 1 {
                return Err(AllocationError::Conflict.into());
            }
        }
    }
    Ok(snapshot)
}

pub(super) async fn read_optional(
    database: &Database,
    collection: &str,
    id: &str,
    session: &mut ClientSession,
) -> Result<Option<Document>, MutationError> {
    let rows: Collection<Document> = majority_collection(database, collection);
    Ok(rows
        .find_one(doc! {"_id":id})
        .session(&mut *session)
        .await?)
}

pub(super) async fn save_state(
    store: &MongoBridgeAllocationStore,
    previous: &ChannelReuseSnapshot,
    next: &ChannelReuseState,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    next.validate(&previous.allocation)?;
    let changed=store.records.update_one(doc! {"_id":&previous.allocation.id,"revision":previous.allocation.revision as i64,
        "channel_reuse":record_bson(&previous.state).map_err(|_| AllocationError::Corrupt)?},
        doc! {"$set":{"channel_reuse":record_bson(next).map_err(|_| AllocationError::Corrupt)?}}).session(&mut *session).await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(())
}

pub(super) async fn replace_fields(
    database: &Database,
    collection: &str,
    id: &str,
    revision: u64,
    mut fields: Document,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    fields.remove("_id");
    let rows: Collection<Document> = majority_collection(database, collection);
    let changed = rows
        .update_one(
            doc! {"_id":id,"revision":revision as i64},
            doc! {"$set":fields},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(())
}

pub(super) async fn event(
    resources: &MongoSandboxStore,
    snapshot: &ChannelReuseSnapshot,
    kind: SandboxEventKind,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    crate::store::sandbox_events::append_in_transaction(
        resources,
        session,
        &snapshot.sandbox.id,
        layer2_core::rfc3339_at(std::time::SystemTime::now()),
        kind,
    )
    .await
    .map_err(|error| match error {
        crate::store::sandbox_events::TransactionEventError::Driver(error) => {
            MutationError::Mongo(error)
        }
        crate::store::sandbox_events::TransactionEventError::Domain(_) => {
            MutationError::Core(AllocationError::Conflict)
        }
    })?;
    Ok(())
}

pub(super) async fn fence_allocation(
    store: &MongoBridgeAllocationStore,
    authority: &AllocationAuthority,
    lease: &BridgeControlPlaneLease,
    allocation: &AllocationRecord,
    session: &mut ClientSession,
) -> Result<Document, MutationError> {
    require_current(authority, lease, allocation)?;
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
    let document = store
        .records
        .find_one(doc! {"_id":&allocation.id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    if &records::decode_record(&document)? != allocation
        || document.contains_key("sandbox_admission")
    {
        return Err(AllocationError::Conflict.into());
    }
    let head_id = allocation.identity.head_id()?;
    let head = store
        .heads
        .find_one(doc! {"_id":&head_id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Conflict)?;
    if records::decode_head(&head, &allocation.identity)? != allocation.id {
        return Err(AllocationError::Conflict.into());
    }
    let changed = store
        .heads
        .update_one(
            doc! {"_id":head_id,"allocation_id":&allocation.id},
            doc! {"$inc":{"channel_reuse_fence":1_i64}},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(document)
}
