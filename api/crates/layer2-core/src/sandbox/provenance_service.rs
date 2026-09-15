use std::{sync::Arc, time::Duration};

use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceAccessSnapshot, ResourceAccessStoreError, ResourceActor,
    ResourceCapability, ResourceTarget,
};
use crate::{AgentId, Caller};

use super::store::{AgentProvenanceStore, SandboxStoreError};
use super::{AgentCreationCategory, AgentCreationOrigin, AgentCreationProvenance, AgentOrigin};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProvenanceReading {
    pub agent_id: AgentId,
    pub created_at: String,
    pub creation_origin: AgentCreationCategory,
    pub details: Option<AgentProvenanceDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProvenanceDetails {
    pub origin: AgentCreationOrigin,
    pub created_by: Option<ResourceActor>,
    pub provisioning_origin: AgentOrigin,
}

pub struct AgentProvenanceService {
    store: Arc<dyn AgentProvenanceStore>,
    access: Arc<ResourceAccessService>,
}

impl AgentProvenanceService {
    pub fn new(
        store: Arc<dyn AgentProvenanceStore>,
        access: Arc<ResourceAccessService>,
    ) -> Arc<Self> {
        Arc::new(Self { store, access })
    }

    pub async fn read(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<AgentProvenanceReading, ResourceAccessError> {
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        let authorized = tokio::time::timeout(
            Duration::from_secs(15),
            self.access.authorize(
                context,
                credential,
                &target,
                ResourceCapability::ViewDetails,
                AccessPurpose::Read,
            ),
        )
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)??;
        self.read_held(authorized, target).await
    }

    pub async fn read_personal(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentProvenanceReading, ResourceAccessError> {
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        let authorized = tokio::time::timeout(
            Duration::from_secs(15),
            self.access
                .authorize_personal(caller, &target, ResourceCapability::ViewDetails),
        )
        .await
        .map_err(|_| ResourceAccessStoreError::Unavailable)??;
        self.read_held(authorized, target).await
    }

    async fn read_held(
        &self,
        authorized: AuthorizedResource,
        target: ResourceTarget,
    ) -> Result<AgentProvenanceReading, ResourceAccessError> {
        let ResourceTarget::Agent { agent_id } = &target else {
            return Err(ResourceAccessStoreError::Corrupt.into());
        };
        // Full source references and creating actors need management authority.
        // Independent operator history must not keep expired org-management data.
        let detailed = authorized.decision.allows(ResourceCapability::ManageBridge);
        let capability = if detailed {
            ResourceCapability::ManageBridge
        } else {
            ResourceCapability::ViewDetails
        };
        let deadline = authorized
            .decision
            .revalidation_deadline(capability)
            .map_or(
                tokio::time::Instant::now() + Duration::from_secs(15),
                |expires| expires.min(tokio::time::Instant::now() + Duration::from_secs(15)),
            );
        let result =
            tokio::time::timeout_at(deadline, self.store.agent_provenance_snapshot(agent_id)).await;
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
        if snapshot.resource.fence() != authorized.snapshot.fence() {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        let record = snapshot
            .creation_provenance(agent_id)
            .map_err(store_error)?;
        require_identity(&record, &authorized.snapshot)?;
        require_identity(&record, &current)?;
        authorized.decision.require(capability)?;
        Ok(AgentProvenanceReading {
            agent_id: record.agent_id,
            created_at: record.agent_created_at,
            creation_origin: record.creation_origin.category(),
            details: detailed.then_some(AgentProvenanceDetails {
                origin: record.creation_origin,
                created_by: record.created_by,
                provisioning_origin: record.provisioning_origin,
            }),
        })
    }
}

fn require_identity(
    record: &AgentCreationProvenance,
    snapshot: &ResourceAccessSnapshot,
) -> Result<(), ResourceAccessError> {
    let target = ResourceTarget::Agent {
        agent_id: record.agent_id.clone(),
    };
    if snapshot.scope(&target)?.owner != record.owner {
        return Err(ResourceAccessStoreError::Conflict.into());
    }
    record
        .validate_against(
            snapshot
                .lineage
                .agent
                .as_ref()
                .ok_or(ResourceAccessStoreError::Corrupt)?,
            &snapshot.lineage.sandbox,
        )
        .map_err(|_| ResourceAccessStoreError::Conflict.into())
}

fn store_error(error: SandboxStoreError) -> ResourceAccessError {
    match error {
        SandboxStoreError::NotFound => ResourceAccessStoreError::NotFound,
        SandboxStoreError::Conflict => ResourceAccessStoreError::Conflict,
        SandboxStoreError::Corrupt(_) => ResourceAccessStoreError::Corrupt,
        _ => ResourceAccessStoreError::Unavailable,
    }
    .into()
}
