//! Permanent Trigger/Agent/command association. All execution proofs re-read it.
use layer2_core::sandbox::agent_io::{AdmittedAgentCommand, TriggerFirstMessageBinding};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::Agent;
use mongodb::bson::{doc, Document};
use mongodb::{ClientSession, Collection};

use super::sandbox_mongo::{
    record_bson, require_version, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};

mod admission;
mod execution;
mod projection;
mod replay;
pub(in crate::store) use admission::attach;
pub(in crate::store) use execution::{proof, proof_in_session, ready_for_claim, require_mutation};
pub(in crate::store) use projection::project_terminal;
pub(in crate::store) const COLLECTION: &str = "trigger_first_message_bindings";
pub(in crate::store) const FIELD: &str = "trigger_first_message_binding";

pub(in crate::store) fn collection(store: &MongoSandboxStore) -> Collection<Document> {
    super::durability::majority_collection(
        &store.client.database(&store.agents.namespace().db),
        COLLECTION,
    )
}

pub(in crate::store) async fn ensure_indexes(
    store: &MongoSandboxStore,
) -> Result<(), layer2_core::StoreError> {
    for (field, name) in [
        ("agent_id", "trigger_first_unique_agent"),
        ("command_id", "trigger_first_unique_command"),
    ] {
        collection(store)
            .create_index(
                mongodb::IndexModel::builder()
                    .keys(doc! {field:1_i32})
                    .options(
                        mongodb::options::IndexOptions::builder()
                            .name(name.to_owned())
                            .unique(true)
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|error| layer2_core::StoreError::Unavailable(error.to_string()))?;
    }
    Ok(())
}

fn document(binding: &TriggerFirstMessageBinding) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "_id": binding.document_key(), "document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "agent_id":binding.agent_id().as_str(), "command_id":binding.receipt().envelope.command_id.as_str(),
        "record":record_bson(binding).map_err(SandboxStoreError::Corrupt)?,
    })
}

fn read(document: &Document) -> Result<TriggerFirstMessageBinding, SandboxStoreError> {
    require_version(document).map_err(SandboxStoreError::Corrupt)?;
    let binding: TriggerFirstMessageBinding =
        super::sandbox_mongo::read_record(document).map_err(SandboxStoreError::Corrupt)?;
    if document != &self::document(&binding)? {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(binding)
}

pub(in crate::store) fn hydrate(
    document: &Document,
    agent: &mut Agent,
) -> Result<(), SandboxStoreError> {
    if let Some(value) = document.get(FIELD) {
        let binding: TriggerFirstMessageBinding =
            mongodb::bson::deserialize_from_bson(value.clone())
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        binding
            .require_agent(agent)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        agent
            .conversation
            .first_message
            .as_mut()
            .ok_or(SandboxStoreError::Conflict)?
            .common_binding = Some(binding);
    }
    Ok(())
}

pub(in crate::store) async fn require_receipt(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Option<TriggerFirstMessageBinding>, SandboxStoreError> {
    replay::receipt(store, agent, session, false).await
}

pub(in crate::store) async fn legacy(
    store: &MongoSandboxStore,
    agent: &Agent,
) -> Result<(), SandboxStoreError> {
    let actual = collection(store)
        .find_one(doc! {"agent_id":agent.id.as_str()})
        .await
        .map_err(unavailable)?;
    if actual.is_some()
        || agent
            .conversation
            .first_message
            .as_ref()
            .is_some_and(|first| first.common_binding.is_some())
    {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(())
}

pub(in crate::store) async fn require_command_receipt(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    replay::command(store, agent, session, false).await
}

pub(in crate::store) async fn require_command_receipt_snapshot(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<(), SandboxStoreError> {
    replay::command(store, agent, session, true).await
}

fn unavailable(error: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

pub(in crate::store) fn require_launch_binding(
    launch: &layer2_core::trigger_domain::Launch,
    actual: Option<&Document>,
) -> Result<Option<layer2_core::trigger_domain::LaunchFirstMessageCommand>, SandboxStoreError> {
    let binding = actual.map(read).transpose()?;
    match binding {
        Some(binding) => {
            binding
                .require_launch(launch)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            let receipt = binding.receipt();
            receipt
                .validate_integrity()
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            if receipt.envelope.owner
                != (layer2_core::resource_access::ResourceOwner::Person {
                    subject: launch.owner.clone(),
                })
            {
                return Err(SandboxStoreError::Corrupt(
                    "Trigger command owner differs".into(),
                ));
            }
            Ok(Some(
                layer2_core::trigger_domain::LaunchFirstMessageCommand {
                    agent_id: receipt.envelope.agent_id.clone(),
                    command_id: receipt.envelope.command_id.clone(),
                    response_id: receipt.response_id.clone().ok_or_else(|| {
                        SandboxStoreError::Corrupt("Trigger command has no response".into())
                    })?,
                },
            ))
        }
        None if launch.input_binding_sha256.is_none() => Ok(None),
        None => Err(SandboxStoreError::Corrupt(
            "Launch command binding is missing".into(),
        )),
    }
}
