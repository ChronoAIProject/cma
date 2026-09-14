use crate::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStore, ResourceAccessStoreError, ResourceLineage,
    ResourceOwner, ResourceScope, ResourceTarget, SharingIsolation,
};
use crate::sandbox::store::InMemorySandboxStore;

#[async_trait::async_trait]
impl ResourceAccessStore for InMemorySandboxStore {
    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        let state = self.lock();
        snapshot(&state, target)
    }
}

fn snapshot(
    state: &super::super::State,
    target: &ResourceTarget,
) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
    let absent = ResourceAccessStoreError::NotFound;
    let corrupt = ResourceAccessStoreError::Corrupt;
    let agent = match target {
        ResourceTarget::Agent { agent_id } => {
            Some(state.agents.get(agent_id).cloned().ok_or(absent.clone())?)
        }
        _ => None,
    };
    let worktree = match target {
        ResourceTarget::Worktree { worktree_id } => Some(
            state
                .worktrees
                .get(worktree_id)
                .cloned()
                .ok_or(absent.clone())?,
        ),
        ResourceTarget::Agent { .. } => Some(
            state
                .worktrees
                .get(&agent.as_ref().ok_or(corrupt.clone())?.worktree_id)
                .cloned()
                .ok_or(corrupt.clone())?,
        ),
        _ => None,
    };
    let workspace = match target {
        ResourceTarget::Workspace { workspace_id } => Some(
            state
                .workspaces
                .get(workspace_id)
                .cloned()
                .ok_or(absent.clone())?,
        ),
        ResourceTarget::Worktree { .. } | ResourceTarget::Agent { .. } => Some(
            state
                .workspaces
                .get(&worktree.as_ref().ok_or(corrupt.clone())?.workspace_id)
                .cloned()
                .ok_or(corrupt.clone())?,
        ),
        _ => None,
    };
    let sandbox_id = match target {
        ResourceTarget::Sandbox { sandbox_id } => sandbox_id.clone(),
        _ => workspace
            .as_ref()
            .ok_or(corrupt.clone())?
            .sandbox_id
            .clone(),
    };
    let sandbox = state.sandboxes.get(&sandbox_id).cloned().ok_or({
        if matches!(target, ResourceTarget::Sandbox { .. }) {
            absent
        } else {
            corrupt
        }
    })?;
    let lineage = ResourceLineage {
        sandbox,
        workspace,
        worktree,
        agent,
    };
    lineage.validate(target)?;
    let selector = ResourceScope {
        sandbox_id: sandbox_id.clone(),
        agent_id: lineage.agent.as_ref().map(|agent| agent.id.clone()),
        owner: ResourceOwner::Person {
            subject: lineage.sandbox.owner.clone(),
        },
        sharing_isolation: SharingIsolation::Unproved,
    };
    let mut snapshot = ResourceAccessSnapshot {
        lineage,
        policy: state.resource_policies.get(&sandbox_id).cloned(),
        shares: state
            .resource_shares
            .values()
            .filter(|receipt| {
                receipt.sandbox_id == sandbox_id
                    && receipt.share.revoked_at.is_none()
                    && receipt.share.selector.covers(&selector)
            })
            .map(|receipt| receipt.share.clone())
            .collect(),
    };
    snapshot
        .shares
        .sort_by(|left, right| left.id.cmp(&right.id));
    snapshot.scope(target)?;
    Ok(snapshot)
}

#[async_trait::async_trait]
impl crate::resource_access::ResourceShareStore for InMemorySandboxStore {
    async fn share_mutation_receipt(
        &self,
        operation_id: &str,
    ) -> Result<Option<crate::resource_access::ShareMutationReceipt>, ResourceAccessStoreError>
    {
        let receipt = self
            .lock()
            .resource_share_commands
            .get(operation_id)
            .cloned();
        if let Some(receipt) = &receipt {
            receipt.validate()?;
        }
        Ok(receipt)
    }

    async fn mutate_share(
        &self,
        request: crate::resource_access::ShareMutation,
    ) -> Result<crate::resource_access::ShareMutationReceipt, ResourceAccessStoreError> {
        use crate::resource_access::{plan_share_mutation, ShareChange};
        request.validate()?;
        let mut state = self.lock();
        if let Some(receipt) = state.resource_share_commands.get(&request.operation_id()) {
            return request.replay(receipt.clone());
        }
        let snapshot = snapshot(&state, &request.target())?;
        let id = match &request.change {
            ShareChange::Create { .. } => &request.proposed_id,
            ShareChange::SetMode { share_id, .. } | ShareChange::Revoke { share_id } => share_id,
        };
        let previous = state.resource_shares.get(id).map(|receipt| &receipt.share);
        let count = state
            .resource_shares
            .values()
            .filter(|receipt| {
                receipt.sandbox_id == snapshot.lineage.sandbox.id
                    && receipt.share.revoked_at.is_none()
            })
            .count();
        let plan = plan_share_mutation(&request, &snapshot, previous, count)?;
        state
            .resource_policies
            .insert(plan.policy.sandbox_id.clone(), plan.policy);
        state
            .resource_shares
            .insert(plan.receipt.share.id.clone(), plan.receipt.clone());
        state
            .resource_share_commands
            .insert(plan.receipt.operation_id.clone(), plan.receipt.clone());
        Ok(plan.receipt)
    }
    async fn resource_share(
        &self,
        id: &str,
    ) -> Result<Option<crate::resource_access::ResourceShare>, ResourceAccessStoreError> {
        Ok(self
            .lock()
            .resource_shares
            .get(id)
            .map(|receipt| receipt.share.clone()))
    }
}
