use super::{ActiveChannelBinding, ChannelUsageEvent, ChannelUsageSummary};
use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceAccessStoreError, ResourceCapability, ResourceTarget,
};
use crate::sandbox::store::{AgentChannelUsageStore, Page, PageRequest, SandboxStoreError};
use crate::{AgentId, Caller};
use std::{sync::Arc, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChannelUsageReading {
    pub agent_id: AgentId,
    pub summary: ChannelUsageSummary,
    /// Binding/source identifiers require current management authority.
    pub active_bindings: Option<Vec<ActiveChannelBinding>>,
    pub history: Option<Page<ChannelUsageEvent>>,
}

pub struct AgentChannelUsageService {
    store: Arc<dyn AgentChannelUsageStore>,
    access: Arc<ResourceAccessService>,
}

impl AgentChannelUsageService {
    pub fn new(
        store: Arc<dyn AgentChannelUsageStore>,
        access: Arc<ResourceAccessService>,
    ) -> Arc<Self> {
        Arc::new(Self { store, access })
    }

    pub async fn read(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        history: Option<&PageRequest>,
    ) -> Result<AgentChannelUsageReading, ResourceAccessError> {
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        let capability = if history.is_some() {
            ResourceCapability::ManageBridge
        } else {
            ResourceCapability::ViewDetails
        };
        let authorized = tokio::time::timeout(
            Duration::from_secs(15),
            self.access.authorize(
                context,
                credential,
                &target,
                capability,
                AccessPurpose::Read,
            ),
        )
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)??;
        self.read_held(authorized, target, history).await
    }

    pub async fn read_personal(
        &self,
        caller: &Caller,
        id: &AgentId,
        history: Option<&PageRequest>,
    ) -> Result<AgentChannelUsageReading, ResourceAccessError> {
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        let capability = if history.is_some() {
            ResourceCapability::ManageBridge
        } else {
            ResourceCapability::ViewDetails
        };
        let authorized = tokio::time::timeout(
            Duration::from_secs(15),
            self.access.authorize_personal(caller, &target, capability),
        )
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)??;
        self.read_held(authorized, target, history).await
    }

    async fn read_held(
        &self,
        authorized: AuthorizedResource,
        target: ResourceTarget,
        history: Option<&PageRequest>,
    ) -> Result<AgentChannelUsageReading, ResourceAccessError> {
        let ResourceTarget::Agent { agent_id } = &target else {
            return Err(ResourceAccessStoreError::Corrupt.into());
        };
        let detailed = authorized.decision.allows(ResourceCapability::ManageBridge);
        let capability = if detailed {
            ResourceCapability::ManageBridge
        } else {
            ResourceCapability::ViewDetails
        };
        if history.is_some() && !detailed {
            return Err(AccessError::Forbidden.into());
        }
        let bound = tokio::time::Instant::now() + Duration::from_secs(15);
        let deadline = authorized
            .decision
            .revalidation_deadline(capability)
            .map_or(bound, |expires| expires.min(bound));
        let result = tokio::time::timeout_at(deadline, async {
            match history {
                Some(page) => self
                    .store
                    .agent_channel_usage_history(agent_id, page)
                    .await
                    .map(|(snapshot, page)| (snapshot, Some(page))),
                None => self
                    .store
                    .agent_channel_usage_snapshot(agent_id)
                    .await
                    .map(|snapshot| (snapshot, None)),
            }
        })
        .await;
        // Even errors or expired buffered reads must not disclose a stale history.
        let current = tokio::time::timeout_at(
            deadline,
            self.access
                .current_read_snapshot(&authorized, &target, capability),
        )
        .await
        .map_err(|_| AccessError::RevalidationRequired)??;
        let (snapshot, history) = result
            .map_err(|_| ResourceAccessStoreError::Unavailable)?
            .map_err(store_error)?;
        snapshot.validate(agent_id).map_err(store_error)?;
        if snapshot.provenance.resource.fence() != authorized.snapshot.fence() {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        super::super::provenance_service::require_identity(
            &snapshot.state.creation,
            &authorized.snapshot,
        )?;
        super::super::provenance_service::require_identity(&snapshot.state.creation, &current)?;
        authorized.decision.require(capability)?;
        Ok(AgentChannelUsageReading {
            agent_id: agent_id.clone(),
            summary: snapshot.state.summary(),
            active_bindings: detailed.then_some(snapshot.state.active_bindings),
            history,
        })
    }
}

fn store_error(error: SandboxStoreError) -> ResourceAccessError {
    match error {
        SandboxStoreError::NotFound => ResourceAccessStoreError::NotFound,
        SandboxStoreError::Conflict => ResourceAccessStoreError::Conflict,
        SandboxStoreError::InvalidPage => ResourceAccessStoreError::Invalid,
        SandboxStoreError::Corrupt(_) => ResourceAccessStoreError::Corrupt,
        _ => ResourceAccessStoreError::Unavailable,
    }
    .into()
}
