//! Replay proves all permanent command artifacts in the caller's single view.
use super::*;

pub(super) async fn receipt(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Option<TriggerFirstMessageBinding>, SandboxStoreError> {
    let actual = find(
        &collection(store),
        doc! {"agent_id":agent.id.as_str()},
        session,
        snapshot,
    )
    .await?
    .as_ref()
    .map(read)
    .transpose()?;
    let expected = agent
        .conversation
        .first_message
        .as_ref()
        .and_then(|first| first.common_binding.as_ref());
    if actual.as_ref() != expected {
        return Err(SandboxStoreError::Corrupt(
            "Trigger first-message receipt and Agent sidecar disagree".into(),
        ));
    }
    if let Some(binding) = &actual {
        binding
            .require_agent(agent)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    }
    Ok(actual)
}

pub(super) async fn command(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<(), SandboxStoreError> {
    let Some(binding) = receipt(store, agent, session, snapshot).await? else {
        return Ok(());
    };
    let row = find(
        &store.agent_commands,
        doc! {"_id":binding.receipt().envelope.command_id.as_str()},
        session,
        snapshot,
    )
    .await?
    .ok_or(SandboxStoreError::Conflict)?;
    let command = super::super::agent_commands::read_command(&row)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    binding
        .require_command(&command)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    require_source(store, agent, &binding, session, snapshot)
        .await
        .map(|_| ())
}

/// Re-read the durable source that authorized this first-message reservation.
pub(super) async fn require_source(
    store: &MongoSandboxStore,
    agent: &Agent,
    binding: &TriggerFirstMessageBinding,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Option<layer2_core::trigger_domain::Launch>, SandboxStoreError> {
    let Some(launch_id) = binding.launch_id() else {
        let row = find(
            &store.agent_creation_provenance,
            doc! {"_id":agent.id.as_str()},
            session,
            snapshot,
        )
        .await?
        .ok_or(SandboxStoreError::Conflict)?;
        let provenance = super::super::agent_provenance::read(&row)?;
        if !binding.is_onboarding()
            || provenance.agent_created_at != agent.created_at
            || provenance.creation_fingerprint != agent.creation_fingerprint
            || provenance.provisioning_origin != agent.origin
            || provenance.owner != binding.receipt().envelope.owner
            || provenance.created_by.as_ref() != Some(&binding.receipt().envelope.actor)
        {
            return Err(SandboxStoreError::Conflict);
        }
        return Ok(None);
    };
    let database = store.client.database(&store.agents.namespace().db);
    let launches =
        super::super::durability::majority_collection::<Document>(&database, "trigger_launches");
    let row = find(
        &launches,
        doc! {"_id":launch_id.as_str()},
        session,
        snapshot,
    )
    .await?
    .ok_or(SandboxStoreError::Conflict)?;
    let launch = super::super::trigger_definitions::decode_launch_aggregate(&row)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    binding
        .require_launch(&launch)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    let protocols = super::super::durability::majority_collection::<Document>(
        &database,
        "trigger_launch_input_protocols",
    );
    let row = find(
        &protocols,
        doc! {"_id":launch_id.as_str()},
        session,
        snapshot,
    )
    .await?;
    super::super::trigger_definitions::require_input_protocol_document(&launch, row)
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
    Ok(Some(launch))
}

async fn find(
    collection: &Collection<Document>,
    filter: Document,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Option<Document>, SandboxStoreError> {
    crate::store::stale_turns::find_with_proof_budget(collection, filter, session, snapshot)
        .await
        .map_err(unavailable)
}
