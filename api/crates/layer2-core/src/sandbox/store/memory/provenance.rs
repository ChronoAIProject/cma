use super::*;
use crate::sandbox::store::{AgentProvenanceSnapshot, AgentProvenanceStore};

#[async_trait::async_trait]
impl AgentProvenanceStore for InMemorySandboxStore {
    async fn agent_provenance_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<AgentProvenanceSnapshot, SandboxStoreError> {
        let state = self.lock();
        let resource = super::resource_access::snapshot(
            &state,
            &crate::resource_access::ResourceTarget::Agent {
                agent_id: id.clone(),
            },
        )
        .map_err(|error| match error {
            crate::resource_access::ResourceAccessStoreError::NotFound => {
                SandboxStoreError::NotFound
            }
            _ => SandboxStoreError::Corrupt(error.to_string()),
        })?;
        Ok(AgentProvenanceSnapshot {
            resource,
            creation: state.agent_creation_provenance.get(id).cloned(),
            first_log: state
                .agent_logs
                .get(id)
                .and_then(|logs| logs.first())
                .cloned(),
        })
    }
}

pub(super) fn creation_record(
    state: &State,
    admission: &crate::sandbox::store::AgentAdmission,
    agent: &Agent,
    parent: &Sandbox,
) -> Result<crate::sandbox::AgentCreationProvenance, SandboxStoreError> {
    let owner = match state.resource_policies.get(&parent.id) {
        Some(policy) => {
            policy
                .personal_owner_of(parent)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            policy.owner.clone()
        }
        None => crate::resource_access::ResourceOwner::Person {
            subject: parent.owner.clone(),
        },
    };
    crate::sandbox::AgentCreationProvenance::for_admission(
        agent,
        parent,
        owner,
        admission.created_by.as_ref(),
    )
}
