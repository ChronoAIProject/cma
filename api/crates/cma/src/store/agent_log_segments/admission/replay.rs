//! A durable reuse binding names the exact immutable publication, even after
//! later Agent changes. Missing artifacts never select a new reuse candidate.
use super::super::{
    publication::{self, Publication, PublicationIdentity},
    writer_reconcile,
};
use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::MongoSandboxStore};
use layer2_core::sandbox::{
    store::{AgentAdmission, SandboxStoreError},
    Agent, AgentLogKind, FirstMessageDelivery,
};
use mongodb::{bson::Document, ClientSession};

pub(in crate::store) async fn require(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    agent: &Agent,
    row: &Document,
    binding: &Document,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<(), TransactionLogError> {
    let Some(value) = binding.get("log_publication") else {
        // Bindings accepted before adoption retain their original contract.
        return Ok(());
    };
    let expected = Publication::read(
        value
            .as_document()
            .ok_or(SandboxStoreError::Conflict)?
            .clone(),
    )?;
    expected.matches_request(
        agent,
        &PublicationIdentity {
            key: format!(
                "reuse:{}",
                crate::store::agent_admission_records::binding_id(admission)?
            ),
            fingerprint: publication::request_digest(&admission.fingerprint()?)?,
            appended_at: admission.event_at.clone(),
        },
    )?;
    if expected.last_seq - expected.expected_seq > 1 {
        return Err(SandboxStoreError::Conflict.into());
    }
    let entries = Box::pin(writer_reconcile::retained(
        store, &expected, agent, row, session, snapshot,
    ))
    .await?;
    if entries.iter().any(|entry| {
        entry.kind
            != (AgentLogKind::FirstMessageChanged {
                state: FirstMessageDelivery::Queued,
                recovery_generation: 0,
            })
    }) {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(())
}
