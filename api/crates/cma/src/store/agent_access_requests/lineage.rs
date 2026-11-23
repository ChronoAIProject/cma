use super::*;
use layer2_core::sandbox::{store::AgentProvenanceSnapshot, SandboxStatus};

pub(in crate::store) async fn validate_lineage_transaction(
    store: &MongoSandboxStore,
    identity: &AgentAccessRequester,
    session: &mut ClientSession,
    require_runtime: bool,
) -> TxResult<AgentProvenanceSnapshot> {
    let snapshot =
        crate::store::agent_provenance::read_snapshot(store, &identity.agent_id, session)
            .await
            .map_err(provenance_error)?;
    let lineage = &snapshot.resource.lineage;
    let agent = lineage.agent.as_ref().ok_or(AgentAccessError::NotFound)?;
    let creation = snapshot
        .creation_provenance(&identity.agent_id)
        .map_err(|_| AgentAccessError::NotFound)?;
    if creation != identity.creation
        || agent.owner != identity.owner
        || lineage.sandbox.owner != identity.owner
        || lineage.sandbox.id != identity.sandbox_id
        || !agent.status.is_live()
        || agent.requested_stop_reason.is_some()
        || agent.conversation.conversation_id.as_deref() != Some(&identity.conversation)
        || lineage.sandbox.status == SandboxStatus::Retired
        || !matches!(creation.owner, layer2_core::resource_access::ResourceOwner::Person { ref subject } if subject == &identity.owner)
        || (require_runtime
            && (lineage.sandbox.runtime.as_ref() != Some(&identity.runtime)
                || agent.conversation.runtime_generation != identity.runtime.provision_generation
                || lineage.sandbox.status != SandboxStatus::Active))
    {
        return Err(AgentAccessError::NotFound.into());
    }
    Ok(snapshot)
}
