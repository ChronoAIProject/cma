//! Resolve connection prerequisites for an exact owner selection without consent.
use super::*;
use crate::store::MongoSandboxStore;
use layer2_core::sandbox::store::AgentProvenanceStore;

pub(super) async fn services(
    store: &MongoSandboxStore,
    policy: &dyn SandboxPolicySource,
    caller: &Caller,
    id: &AgentId,
    services: &[String],
) -> Result<Vec<ServiceId>, ConnectionError> {
    if !caller.authorization.allows(Capability::UseOwnSandboxes) {
        return Err(ConnectionError::Forbidden);
    }
    layer2_core::sandbox::agent_access::validate_access_services(services)
        .map_err(|_| ConnectionError::Changed)?;
    let mut selected = services
        .iter()
        .cloned()
        .map(ServiceId::new)
        .collect::<Vec<_>>();
    selected.sort();
    if selected.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ConnectionError::Changed);
    }
    let snapshot = store
        .agent_provenance_snapshot(id)
        .await
        .map_err(|error| match error {
            layer2_core::sandbox::store::SandboxStoreError::NotFound => ConnectionError::NotFound,
            _ => ConnectionError::Unavailable,
        })?;
    let creation = snapshot
        .creation_provenance(id)
        .map_err(|_| ConnectionError::NotFound)?;
    if !matches!(&creation.owner, layer2_core::resource_access::ResourceOwner::Person { subject } if subject == &caller.subject)
    {
        return Err(ConnectionError::NotFound);
    }
    let agent = snapshot
        .resource
        .lineage
        .agent
        .as_ref()
        .filter(|agent| {
            agent.owner == caller.subject
                && agent.status.is_live()
                && agent.requested_stop_reason.is_none()
        })
        .ok_or(ConnectionError::NotFound)?;
    let sandbox = Some(&snapshot.resource.lineage.sandbox)
        .filter(|sandbox| {
            sandbox.owner == caller.subject
                && sandbox.id == agent.sandbox_id
                && sandbox.status != SandboxStatus::Retired
                && sandbox.credential.source.is_managed()
        })
        .ok_or(ConnectionError::NotFound)?;
    let baseline = policy
        .current_sandbox_policy()
        .await
        .map_err(|_| ConnectionError::Unavailable)?
        .baseline_services;
    if !baseline.iter().all(|service| selected.contains(service)) {
        return Err(ConnectionError::Changed);
    }
    // Resolving this target establishes only connection prerequisites;
    // it never records consent, issues a key or changes Sandbox scope.
    Ok(sandbox
        .profile
        .content
        .connection_services_for_grants(&selected))
}

#[cfg(test)]
#[path = "targets_scope_tests.rs"]
mod tests;
