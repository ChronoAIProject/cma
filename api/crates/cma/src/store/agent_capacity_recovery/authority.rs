//! Bind an existing resource decision to this transaction's exact ancestry.
use layer2_core::resource_access::{
    AuthorizedResource, ResourceAccessSnapshot, ResourceCapability, ResourceTarget,
};
use layer2_core::sandbox::{
    agent_capacity_recovery::CapacityRecoveryContext, store::SandboxStoreError,
};
use layer2_core::AgentId;
use mongodb::{bson::doc, ClientSession};

use crate::store::{
    agent_log_integrity::TransactionLogError,
    resource_access::{read_execution_snapshot, ReadError},
    sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION},
};

pub(super) struct Authority {
    resource: ResourceAccessSnapshot,
}

impl Authority {
    pub(super) fn context(&self) -> Result<CapacityRecoveryContext<'_>, TransactionLogError> {
        let lineage = &self.resource.lineage;
        Ok(CapacityRecoveryContext {
            agent: lineage.agent.as_ref().ok_or(SandboxStoreError::Conflict)?,
            sandbox: &lineage.sandbox,
            workspace: lineage
                .workspace
                .as_ref()
                .ok_or(SandboxStoreError::Conflict)?,
            worktree: lineage
                .worktree
                .as_ref()
                .ok_or(SandboxStoreError::Conflict)?,
        })
    }
}

pub(super) async fn capture(
    store: &MongoSandboxStore,
    authorized: &AuthorizedResource,
    id: &AgentId,
    session: &mut ClientSession,
) -> Result<Authority, TransactionLogError> {
    require(authorized)?;
    let target = ResourceTarget::Agent {
        agent_id: id.clone(),
    };
    let current = read_execution_snapshot(store, &target, session)
        .await
        .map_err(|error| match error {
            ReadError::Driver(error) => TransactionLogError::Driver(error),
            ReadError::Domain(_) => TransactionLogError::Domain(SandboxStoreError::Conflict),
        })?;
    if current.fence() != authorized.snapshot.fence()
        || current
            .scope(&target)
            .map_err(|_| SandboxStoreError::Conflict)?
            != authorized
                .snapshot
                .scope(&target)
                .map_err(|_| SandboxStoreError::Conflict)?
        || current.lineage != authorized.snapshot.lineage
        || current.shares != authorized.snapshot.shares
    {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(Authority { resource: current })
}

pub(super) fn require(authorized: &AuthorizedResource) -> Result<(), TransactionLogError> {
    authorized
        .decision
        .require(ResourceCapability::OperateAgent)
        .map_err(|_| {
            SandboxStoreError::InvalidTransaction(
                "current capacity recovery authorization is required",
            )
            .into()
        })
}

/// Resource-policy mutations also fence the Sandbox. These real parent writes
/// serialize stop/retirement, policy changes and home/runtime replacement with
/// the Agent CAS owned solely by writer_commit::persist.
pub(super) async fn fence_parents(
    store: &MongoSandboxStore,
    authority: &Authority,
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    let context = authority.context()?;
    for (collection, id, revision) in [
        (
            &store.sandboxes,
            context.sandbox.id.as_str(),
            context.sandbox.revision,
        ),
        (
            &store.workspaces,
            context.workspace.id.as_str(),
            context.workspace.revision,
        ),
        (
            &store.worktrees,
            context.worktree.id.as_str(),
            context.worktree.revision,
        ),
    ] {
        let revision = signed(revision, "capacity recovery parent revision")
            .map_err(SandboxStoreError::Corrupt)?;
        let result = collection.update_one(doc! {
            "_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": revision, "record.revision": revision, "owner": context.agent.owner.as_str(),
        }, doc! {"$inc": {"capacity_recovery_fence": 1_i64}})
            .session(&mut *session).await.map_err(TransactionLogError::Driver)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict.into());
        }
    }
    Ok(())
}
