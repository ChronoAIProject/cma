//! Personal inventory uses the same per-resource policy with a batched release fence.
use super::*;
use crate::sandbox::store::{Page, PageRequest};
use crate::sandbox::{Agent, AgentListQuery};
use crate::Caller;

impl ResourceAccessService {
    pub(crate) async fn personal_inventory_parents(
        &self,
        caller: &Caller,
        targets: &[ResourceTarget],
    ) -> Result<Option<Vec<(ResourceTarget, AuthorizedResource)>>, ResourceAccessError> {
        if targets.is_empty() {
            return Ok(None);
        }
        let Some(current) = self
            .store
            .personal_agent_inventory_fence(&[], targets)
            .await?
        else {
            return Ok(None);
        };
        if !current.agents.is_empty() || current.parents.len() != targets.len() {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        targets
            .iter()
            .cloned()
            .zip(current.parents)
            .map(|(target, snapshot)| {
                let ResourceOwner::Person { subject } = snapshot.scope(&target)?.owner else {
                    return Err(AccessError::NotFound.into());
                };
                let decision = AccessDecision::personal(caller, &subject);
                decision.require(ResourceCapability::ViewDetails)?;
                Ok((
                    target,
                    AuthorizedResource {
                        snapshot,
                        decision,
                        caller: caller.clone(),
                        binding: None,
                    },
                ))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    /// Reuse complete current snapshots for a personal provenance scan, including
    /// its hidden lookahead. Each original decision keeps its own policy fence.
    pub(crate) async fn personal_agent_fences(
        &self,
        authorized: &[AuthorizedResource],
        parents: &[(ResourceTarget, AuthorizedResource)],
    ) -> Result<Option<Vec<ResourceAccessSnapshot>>, ResourceAccessError> {
        let agents = authorized
            .iter()
            .map(|held| {
                held.snapshot
                    .lineage
                    .agent
                    .clone()
                    .ok_or(ResourceAccessStoreError::Corrupt)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let current = if parents.is_empty() {
            let Some(current) = self.store.personal_agent_page_fence(&agents).await? else {
                return Ok(None);
            };
            current
        } else {
            let targets = parents
                .iter()
                .map(|(target, _)| target.clone())
                .collect::<Vec<_>>();
            let current = self
                .store
                .personal_agent_inventory_fence(&agents, &targets)
                .await?
                .ok_or(ResourceAccessStoreError::Unavailable)?;
            if current.parents.len() != parents.len() {
                return Err(ResourceAccessStoreError::Conflict.into());
            }
            for ((target, held), snapshot) in parents.iter().zip(&current.parents) {
                self.validate_current_read_snapshot(
                    held,
                    target,
                    ResourceCapability::ViewDetails,
                    snapshot,
                )
                .await?;
            }
            current.agents
        };
        if current.len() != authorized.len() {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        for ((held, agent), snapshot) in authorized.iter().zip(agents).zip(&current) {
            if snapshot.lineage.agent.as_ref() != Some(&agent) {
                return Err(ResourceAccessStoreError::Conflict.into());
            }
            self.validate_current_read_snapshot(
                held,
                &ResourceTarget::Agent { agent_id: agent.id },
                ResourceCapability::ViewDetails,
                snapshot,
            )
            .await?;
        }
        Ok(Some(current))
    }

    /// Reuse the same bounded final-fence boundary for organization inventory.
    /// Organization policy evaluation remains in the inventory service.
    pub(crate) async fn organization_agent_fences(
        &self,
        authorized: &[AuthorizedResource],
    ) -> Result<Option<Vec<ResourceAccessSnapshot>>, ResourceAccessError> {
        let agents = authorized
            .iter()
            .map(|held| {
                held.snapshot
                    .lineage
                    .agent
                    .clone()
                    .ok_or(ResourceAccessStoreError::Corrupt)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.store
            .organization_agent_page_fence(&agents)
            .await
            .map_err(Into::into)
    }

    pub(crate) async fn personal_agent_page(
        &self,
        caller: &Caller,
        query: &AgentListQuery,
        request: &PageRequest,
        page: &Page<Agent>,
    ) -> Result<bool, ResourceAccessError> {
        let Some(current) = self.store.personal_agent_page_fence(&page.items).await? else {
            return Ok(false);
        };
        if page.items.len() > usize::from(request.limit)
            || (page.next.is_some() && page.items.len() != usize::from(request.limit))
        {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        if current.len() != page.items.len() {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        let mut previous = request.cursor.as_deref();
        for (selected, snapshot) in page.items.iter().zip(&current) {
            let agent = personal_agent(caller, snapshot)?;
            // Selection is not authority. Release only the exact selected Agent
            // under this fresh, complete parent/policy/share snapshot.
            if agent != selected {
                return Err(ResourceAccessStoreError::Conflict.into());
            }
            if !query.matches(agent) || previous.is_some_and(|id| agent.id.as_str() <= id) {
                return Err(ResourceAccessStoreError::Corrupt.into());
            }
            previous = Some(agent.id.as_str());
        }
        if page.next.as_deref() != page.next.as_ref().and(previous) {
            return Err(ResourceAccessStoreError::Corrupt.into());
        }
        Ok(true)
    }
}

#[tracing::instrument(
    level = "debug",
    target = "cma.agent_inventory",
    name = "authorize",
    skip_all
)]
fn personal_agent<'a>(
    caller: &Caller,
    snapshot: &'a ResourceAccessSnapshot,
) -> Result<&'a Agent, ResourceAccessError> {
    let agent = snapshot
        .lineage
        .agent
        .as_ref()
        .ok_or(ResourceAccessStoreError::Corrupt)?;
    let target = ResourceTarget::Agent {
        agent_id: agent.id.clone(),
    };
    let ResourceOwner::Person { subject } = snapshot.scope(&target)?.owner else {
        return Err(AccessError::NotFound.into());
    };
    AccessDecision::personal(caller, &subject).require(ResourceCapability::ViewDetails)?;
    Ok(agent)
}
