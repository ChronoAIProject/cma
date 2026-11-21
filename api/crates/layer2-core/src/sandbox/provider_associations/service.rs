use super::{association_page, AgentProviderAssociationReading, AgentProviderAssociationStore};
use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceAccessStoreError, ResourceCapability, ResourceTarget,
};
use crate::sandbox::store::{PageRequest, SandboxStoreError};
use crate::{AgentId, Caller};
use std::{sync::Arc, time::Duration};

pub struct AgentProviderAssociationService {
    store: Arc<dyn AgentProviderAssociationStore>,
    access: Arc<ResourceAccessService>,
}
impl AgentProviderAssociationService {
    pub fn new(
        store: Arc<dyn AgentProviderAssociationStore>,
        access: Arc<ResourceAccessService>,
    ) -> Arc<Self> {
        Arc::new(Self { store, access })
    }
    pub async fn read(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        page: &PageRequest,
    ) -> Result<AgentProviderAssociationReading, ResourceAccessError> {
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        let authorized = tokio::time::timeout(
            Duration::from_secs(15),
            self.access.authorize(
                context,
                credential,
                &target,
                ResourceCapability::ManageBridge,
                AccessPurpose::Read,
            ),
        )
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)??;
        self.read_held(authorized, target, page).await
    }
    pub async fn read_personal(
        &self,
        caller: &Caller,
        id: &AgentId,
        page: &PageRequest,
    ) -> Result<AgentProviderAssociationReading, ResourceAccessError> {
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        let authorized = tokio::time::timeout(
            Duration::from_secs(15),
            self.access
                .authorize_personal(caller, &target, ResourceCapability::ManageBridge),
        )
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)??;
        self.read_held(authorized, target, page).await
    }
    async fn read_held(
        &self,
        authorized: AuthorizedResource,
        target: ResourceTarget,
        page: &PageRequest,
    ) -> Result<AgentProviderAssociationReading, ResourceAccessError> {
        let ResourceTarget::Agent { agent_id } = &target else {
            return Err(ResourceAccessStoreError::Corrupt.into());
        };
        let capability = ResourceCapability::ManageBridge;
        let owner = authorized.snapshot.lineage.sandbox.owner.clone();
        let bound = tokio::time::Instant::now() + Duration::from_secs(15);
        let deadline = authorized
            .decision
            .revalidation_deadline(capability)
            .map_or(bound, |expires| expires.min(bound));
        let result = tokio::time::timeout_at(
            deadline,
            self.store
                .agent_provider_associations(&owner, agent_id, page),
        )
        .await;
        // Revalidate even a failed or buffered read before disclosing its result.
        let current = tokio::time::timeout_at(
            deadline,
            self.access
                .current_read_snapshot(&authorized, &target, capability),
        )
        .await
        .map_err(|_| AccessError::RevalidationRequired)??;
        let snapshot = result
            .map_err(|_| ResourceAccessStoreError::Unavailable)?
            .map_err(store_error)?;
        if snapshot.provenance.resource.fence() != authorized.snapshot.fence() {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        let creation = snapshot
            .provenance
            .creation_provenance(agent_id)
            .map_err(store_error)?;
        crate::sandbox::provenance_service::require_identity(&creation, &authorized.snapshot)?;
        crate::sandbox::provenance_service::require_identity(&creation, &current)?;
        // Worktree recovery also changes an association's exact lineage. Do not
        // disclose an old buffered relationship as belonging to its replacement.
        for resource in [&authorized.snapshot, &current] {
            let agent = resource
                .lineage
                .agent
                .as_ref()
                .ok_or(ResourceAccessStoreError::Corrupt)?;
            if snapshot
                .provenance
                .resource
                .lineage
                .agent
                .as_ref()
                .is_none_or(|read| read.worktree_id != agent.worktree_id)
            {
                return Err(ResourceAccessStoreError::Conflict.into());
            }
        }
        authorized.decision.require(capability)?;
        association_page(&snapshot, &owner, agent_id, page).map_err(store_error)
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
