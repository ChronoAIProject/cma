use crate::resource_access::{ResourceAccessSnapshot, ResourceTarget};
use crate::AgentId;

use super::SandboxStoreError;
use crate::sandbox::{AgentCreationProvenance, AgentLogEntry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProvenanceSnapshot {
    pub resource: ResourceAccessSnapshot,
    pub creation: Option<AgentCreationProvenance>,
    pub first_log: Option<AgentLogEntry>,
}

impl AgentProvenanceSnapshot {
    pub fn creation_provenance(
        &self,
        id: &AgentId,
    ) -> Result<AgentCreationProvenance, SandboxStoreError> {
        let target = ResourceTarget::Agent {
            agent_id: id.clone(),
        };
        let owner = self
            .resource
            .scope(&target)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?
            .owner;
        let agent = self
            .resource
            .lineage
            .agent
            .as_ref()
            .ok_or(SandboxStoreError::NotFound)?;
        match &self.creation {
            Some(record) => {
                record.validate_against(agent, &self.resource.lineage.sandbox)?;
                if record.owner != owner {
                    return Err(SandboxStoreError::Corrupt(
                        "Agent provenance owner kind changed".into(),
                    ));
                }
                Ok(record.clone())
            }
            None => AgentCreationProvenance::from_legacy(
                agent,
                &self.resource.lineage.sandbox,
                owner,
                self.first_log.as_ref(),
            ),
        }
    }
}

#[async_trait::async_trait]
pub trait AgentProvenanceStore: Send + Sync {
    /// Read creation evidence, complete ancestry and current ownership under one
    /// snapshot. This is internal data; the service must authorize disclosure.
    async fn agent_provenance_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<AgentProvenanceSnapshot, SandboxStoreError>;
}
