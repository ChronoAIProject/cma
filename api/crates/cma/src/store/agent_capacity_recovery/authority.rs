//! Bind an existing resource decision to this transaction's exact ancestry.
use layer2_core::resource_access::{
    AuthorizedResource, ResourceAccessSnapshot, ResourceCapability, ResourceLineage, ResourceTarget,
};
use layer2_core::sandbox::{
    agent_capacity_recovery::CapacityRecoveryContext, store::SandboxStoreError,
};
use layer2_core::AgentId;
use mongodb::bson::doc;

use crate::store::{
    agent_log_integrity::TransactionLogError,
    agent_log_segments::transaction_scope::TransactionScope,
    agents::read_agent,
    resource_access::{read_inventory_policy, records::read_share},
    sandbox_mongo::{signed, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION},
    sandboxes::read_sandbox,
    workspaces::read_workspace,
    worktrees::read_worktree,
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
    scope: &mut TransactionScope<'_>,
) -> Result<Authority, TransactionLogError> {
    require(authorized)?;
    let target = ResourceTarget::Agent {
        agent_id: id.clone(),
    };
    let agent = read_agent(
        &scope
            .find_one(&store.agents, doc! {"_id": id.as_str()})
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or(SandboxStoreError::NotFound)?,
    )?;
    let sandbox = read_sandbox(
        &scope
            .find_one(&store.sandboxes, doc! {"_id": agent.sandbox_id.as_str()})
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or(SandboxStoreError::Conflict)?,
    )?;
    let workspace = read_workspace(
        &scope
            .find_one(&store.workspaces, doc! {"_id": agent.workspace_id.as_str()})
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or(SandboxStoreError::Conflict)?,
    )?;
    let worktree = read_worktree(
        &scope
            .find_one(&store.worktrees, doc! {"_id": agent.worktree_id.as_str()})
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or(SandboxStoreError::Conflict)?,
    )?;
    let policy = scope
        .find_one(&store.resource_policies, doc! {"_id": sandbox.id.as_str()})
        .await
        .map_err(TransactionLogError::Driver)?
        .as_ref()
        .map(read_inventory_policy)
        .transpose()
        .map_err(|_| SandboxStoreError::Conflict)?;
    if policy != authorized.snapshot.policy {
        return Err(SandboxStoreError::Conflict.into());
    }
    // Every share mutation advances the policy revision and fences the Sandbox.
    // Read each previously authorized share as well; the bounded policy snapshot
    // and exact revision equality also reject additions outside this list.
    let mut shares = Vec::with_capacity(authorized.snapshot.shares.len());
    if authorized.snapshot.shares.len() > layer2_core::resource_access::MAX_ACTIVE_RESOURCE_SHARES {
        return Err(SandboxStoreError::Conflict.into());
    }
    for share in &authorized.snapshot.shares {
        let row = scope
            .find_one(&store.resource_shares, doc! {"_id": &share.id})
            .await
            .map_err(TransactionLogError::Driver)?
            .ok_or(SandboxStoreError::Conflict)?;
        shares.push(read_share(&sandbox.id, &row).map_err(|_| SandboxStoreError::Conflict)?);
    }
    let current = ResourceAccessSnapshot {
        lineage: ResourceLineage {
            agent: Some(agent),
            sandbox,
            workspace: Some(workspace),
            worktree: Some(worktree),
        },
        policy,
        shares,
    };
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
    scope: &mut TransactionScope<'_>,
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
        let result = scope.update_one(collection, doc! {
            "_id": id, "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": revision, "record.revision": revision, "owner": context.agent.owner.as_str(),
        }, doc! {"$inc": {"capacity_recovery_fence": 1_i64}})
            .await.map_err(TransactionLogError::Driver)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict.into());
        }
    }
    Ok(())
}
