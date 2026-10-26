use crate::resource_access::{
    ResourceAccessSnapshot, ResourceAccessStore, ResourceAccessStoreError, ResourceLineage,
    ResourceOwner, ResourceScope, ResourceSelector, ResourceTarget, SharingIsolation,
};
use crate::sandbox::store::InMemorySandboxStore;

#[async_trait::async_trait]
impl ResourceAccessStore for InMemorySandboxStore {
    async fn organization_sandboxes(
        &self,
        organization: &crate::SubjectId,
        query: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        crate::sandbox::store::PageRequest::new(query.page.limit, query.page.cursor.clone())
            .map_err(|_| ResourceAccessStoreError::Invalid)?;
        let state = self.lock();
        let mut candidates: Vec<_> = state.sandboxes.values().filter(|sandbox| {
            if query.status.is_some_and(|status| sandbox.status != status)
                || query.page.cursor.as_deref().is_some_and(|id| sandbox.id.as_str() <= id) {
                return false;
            }
            let owned = state.resource_policies.get(&sandbox.id).is_some_and(|policy| {
                matches!(&policy.owner, ResourceOwner::Organization { subject } if subject == organization)
            });
            owned || state.resource_shares.values().any(|receipt| {
                receipt.sandbox_id == sandbox.id
                    && receipt.share.target_org_id == *organization
                    && receipt.share.revoked_at.is_none()
                    && matches!(&receipt.share.selector, ResourceSelector::Sandbox { sandbox_id } if sandbox_id == &sandbox.id)
            })
        }).collect();
        candidates.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        candidates
            .into_iter()
            .take(usize::from(query.page.limit) + 1)
            .map(|sandbox| {
                let value = snapshot(
                    &state,
                    &ResourceTarget::Sandbox {
                        sandbox_id: sandbox.id.clone(),
                    },
                )?;
                value.organization_sandbox(organization)?;
                Ok(value)
            })
            .collect()
    }

    async fn shared_sandboxes(
        &self,
        recipient: &crate::SubjectId,
        organizations: &[crate::SubjectId],
        query: &crate::sandbox::OwnerSandboxQuery,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        crate::sandbox::store::PageRequest::new(query.page.limit, query.page.cursor.clone())
            .map_err(|_| ResourceAccessStoreError::Invalid)?;
        let state = self.lock();
        let mut candidates: Vec<_> = state.sandboxes.values().filter(|sandbox| {
            query.status.is_none_or(|status| sandbox.status == status)
                && query.page.cursor.as_deref().is_none_or(|cursor| sandbox.id.as_str() > cursor)
                && state.resource_shares.values().any(|receipt| {
                    receipt.sandbox_id == sandbox.id
                        && organizations.contains(&receipt.share.target_org_id)
                        && receipt.share.revoked_at.is_none()
                        && !matches!(&receipt.share.owner, ResourceOwner::Person { subject } if subject == recipient)
                        && matches!(&receipt.share.selector, ResourceSelector::Sandbox { sandbox_id } if sandbox_id == &sandbox.id)
                })
        }).collect();
        candidates.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
        candidates
            .into_iter()
            .take(usize::from(query.page.limit) + 1)
            .map(|sandbox| {
                let value = snapshot(
                    &state,
                    &ResourceTarget::Sandbox {
                        sandbox_id: sandbox.id.clone(),
                    },
                )?;
                value.shared_sandbox(recipient, organizations)?;
                Ok(value)
            })
            .collect()
    }

    async fn organization_agents(
        &self,
        organization: &crate::SubjectId,
        query: &crate::sandbox::AgentListQuery,
        page: &crate::sandbox::store::PageRequest,
    ) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
        organization_agent_snapshots(&self.lock(), organization, query, page)
    }

    async fn resource_access_snapshot(
        &self,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessStoreError> {
        let state = self.lock();
        snapshot(&state, target)
    }
}

pub(super) fn organization_agent_snapshots(
    state: &super::super::State,
    organization: &crate::SubjectId,
    query: &crate::sandbox::AgentListQuery,
    page: &crate::sandbox::store::PageRequest,
) -> Result<Vec<ResourceAccessSnapshot>, ResourceAccessStoreError> {
    crate::sandbox::store::PageRequest::new(page.limit, page.cursor.clone())
        .map_err(|_| ResourceAccessStoreError::Invalid)?;
    let mut candidates: Vec<_> = state.agents.values().filter(|agent| {
            if !query.matches(agent)
                || page.cursor.as_deref().is_some_and(|cursor| agent.id.as_str() <= cursor)
            {
                return false;
            }
            let owned = state.resource_policies.get(&agent.sandbox_id).is_some_and(|policy| {
                matches!(&policy.owner, ResourceOwner::Organization { subject } if subject == organization)
            });
            owned || state.resource_shares.values().any(|receipt| {
                receipt.sandbox_id == agent.sandbox_id
                    && receipt.share.target_org_id == *organization
                    && receipt.share.revoked_at.is_none()
                    && match &receipt.share.selector {
                        ResourceSelector::Sandbox { sandbox_id } => sandbox_id == &agent.sandbox_id,
                        ResourceSelector::Agent { agent_id } => agent_id == &agent.id,
                    }
            })
        }).collect();
    candidates.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
    candidates
        .into_iter()
        .take(usize::from(page.limit) + 1)
        .map(|agent| {
            let value = snapshot(
                state,
                &ResourceTarget::Agent {
                    agent_id: agent.id.clone(),
                },
            )?;
            value.organization_agent(organization)?;
            Ok(value)
        })
        .collect()
}

pub(super) fn snapshot(
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
    if matches!(target, ResourceTarget::Worktree { .. }) {
        if let Some(id) = lineage
            .worktree
            .as_ref()
            .and_then(|tree| tree.agent_id.as_ref())
        {
            lineage.validate_worktree_agent(
                state
                    .agents
                    .get(id)
                    .ok_or(ResourceAccessStoreError::Corrupt)?,
            )?;
        }
    }
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
            ShareChange::Create { .. } | ShareChange::CreateLink { .. } => &request.proposed_id,
            ShareChange::SetMode { share_id, .. }
            | ShareChange::Revoke { share_id }
            | ShareChange::RedeemLink { share_id, .. } => share_id,
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

#[cfg(any(test, feature = "test-support"))]
impl InMemorySandboxStore {
    #[doc(hidden)]
    pub fn set_resource_policy_for_test(
        &self,
        policy: crate::resource_access::ResourcePolicyRecord,
    ) {
        self.lock()
            .resource_policies
            .insert(policy.sandbox_id.clone(), policy);
    }
}
