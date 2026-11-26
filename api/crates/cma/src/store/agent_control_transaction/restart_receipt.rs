//! An accepted restart outlives the Agent's current control workflow.
use layer2_core::sandbox::store::{AgentControlStoreError, AgentRestartAcceptance};
use layer2_core::OperationClaim;
use mongodb::bson::{doc, spec::BinarySubtype, Binary, Document};
use mongodb::Collection;
use sha2::{Digest, Sha256};

use crate::store::{
    durability::majority_collection,
    operation::OPERATIONS,
    sandbox_mongo::{MongoSandboxStore, DOCUMENT_SCHEMA_VERSION},
};

pub(super) fn records(store: &MongoSandboxStore) -> Collection<Document> {
    majority_collection(
        &store.client.database(&store.agents.namespace().db),
        OPERATIONS,
    )
}

pub(super) fn key(claim: &OperationClaim) -> Document {
    doc! {
        "subject": claim.subject().as_str(),
        "operation": claim.operation(),
        "key": claim.key(),
    }
}

pub(super) fn operation_id(claim: &OperationClaim) -> String {
    let mut hash = Sha256::new();
    hash.update(b"cma.agent.restart.operation.v1\0");
    for value in [claim.subject().as_str(), claim.operation(), claim.key()] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value.as_bytes());
    }
    format!("arst_{:x}", hash.finalize())
}

pub(super) fn encode(
    claim: &OperationClaim,
    acceptance: &AgentRestartAcceptance,
) -> Result<Document, AgentControlStoreError> {
    let outcome = serde_json::to_vec(acceptance).map_err(|_| corrupt())?;
    Ok(doc! {
        "_id": key(claim),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "fingerprint": binary(claim.fingerprint().to_vec()),
        "outcome": binary(outcome),
    })
}

pub(super) fn decode(
    claim: &OperationClaim,
    row: &Document,
) -> Result<AgentRestartAcceptance, AgentControlStoreError> {
    if row.get_document("_id").ok() != Some(&key(claim))
        || row.get_i64("document_schema_version").ok() != Some(DOCUMENT_SCHEMA_VERSION)
    {
        return Err(corrupt());
    }
    if row
        .get_binary_generic("fingerprint")
        .map_err(|_| corrupt())?
        .as_slice()
        != claim.fingerprint()
    {
        return Err(AgentControlStoreError::Conflict);
    }
    let acceptance: AgentRestartAcceptance =
        serde_json::from_slice(row.get_binary_generic("outcome").map_err(|_| corrupt())?)
            .map_err(|_| corrupt())?;
    if acceptance.operation_id != operation_id(claim) {
        return Err(corrupt());
    }
    Ok(acceptance)
}

fn binary(bytes: Vec<u8>) -> Binary {
    Binary {
        subtype: BinarySubtype::Generic,
        bytes,
    }
}

fn corrupt() -> AgentControlStoreError {
    AgentControlStoreError::Corrupt("stored Agent restart acceptance is invalid".into())
}
