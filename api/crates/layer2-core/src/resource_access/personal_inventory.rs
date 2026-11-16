//! Personal inventory uses the same per-resource policy with a batched release fence.
use super::*;
use crate::sandbox::store::{Page, PageRequest};
use crate::sandbox::{Agent, AgentListQuery};
use crate::Caller;

impl ResourceAccessService {
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
