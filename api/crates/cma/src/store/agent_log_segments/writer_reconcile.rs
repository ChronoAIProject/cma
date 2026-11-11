//! Exact receipt reconciliation in one majority-visible snapshot. A read can
//! prove Recorded, never KnownAbort. Missing/unavailable evidence stays Unknown.
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry};
use mongodb::{
    bson::{doc, Binary},
    options::ReadConcern,
    ClientSession,
};

use super::{
    corrupt, digest,
    publication::{self, Publication, PublicationIdentity},
    read_format, read_head, visit_range, LogRange, Seal,
};
use crate::store::{
    agent_log_integrity::{ensure_append_batch_size, TransactionLogError},
    agents::read_agent,
    sandbox_mongo::MongoSandboxStore,
};

pub(in crate::store) enum Outcome {
    Recorded(Box<Publication>),
    Unknown,
}

/// `expected` and `logs` are the original attempted result, not a reconstructed
/// result against the latest lease. Related domain receipts are also checked by
/// the caller before acknowledging its external effect/checkpoint.
/// The caller supplies a snapshot session; this helper neither retries nor writes.
pub(in crate::store) async fn reconcile(
    store: &MongoSandboxStore,
    operation: &PublicationIdentity,
    expected: &Agent,
    logs: &[AgentLogEntry],
    result: &Binary,
    session: &mut ClientSession,
) -> Result<Outcome, SandboxStoreError> {
    operation.validate()?;
    ensure_append_batch_size(logs.len())?;
    let incarnation = digest::incarnation(expected);
    let receipt = store
        .segment_logs
        .publications
        .find_one(doc! {
            "_id":publication::identity(expected.id.as_str(), &incarnation, &operation.key),
        })
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(driver)?;
    let Some(row) = receipt else {
        return Ok(Outcome::Unknown);
    };
    let receipt = Publication::read(row)?;
    receipt.matches_request(expected, operation)?;
    if receipt.result_digest != *result
        || u64::try_from(receipt.result_revision).ok() != Some(expected.revision)
        || u64::try_from(receipt.last_seq).ok() != Some(expected.conversation.last_seq)
        || receipt.last_seq - receipt.expected_seq != logs.len() as i64
        || logs.iter().enumerate().any(|(offset, entry)| {
            entry.agent_id != expected.id
                || u64::try_from(receipt.first_seq + offset as i64).ok() != Some(entry.seq)
        })
    {
        return Err(corrupt());
    }
    let row = store
        .agents
        .find_one(doc! {"_id":expected.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(driver)?
        .ok_or_else(corrupt)?;
    if read_format(&row)?.is_none() {
        return Err(corrupt());
    }
    let current = read_agent(&row)?;
    let head = read_head(store, &row, session, true).await.map_err(error)?;
    if head.incarnation != incarnation
        || current.revision < expected.revision
        || head.last_seq < receipt.last_seq
        || head.revision < receipt.head_revision
        || head.active_ordinal < receipt.active_ordinal
        || (head.revision == receipt.head_revision && head.last_seq != receipt.last_seq)
        || (head.active_ordinal == receipt.active_ordinal
            && head.previous_seal != receipt.previous_seal)
    {
        return Err(corrupt());
    }
    if receipt.active_ordinal > 0 {
        let ordinal = receipt.active_ordinal - 1;
        let seal = store
            .segment_logs
            .seals
            .find_one(doc! {"_id":Seal::identity(&head.agent_id, &head.incarnation, ordinal)})
            .read_concern(ReadConcern::snapshot())
            .session(&mut *session)
            .await
            .map_err(driver)?
            .ok_or_else(corrupt)?;
        let seal = Seal::read(seal)?;
        if seal.agent_id != head.agent_id
            || seal.incarnation != head.incarnation
            || seal.ordinal != ordinal
            || receipt.previous_seal.as_ref() != Some(&seal.seal_digest)
            || receipt.expected_seq < seal.end_seq
        {
            return Err(corrupt());
        }
    }
    if !logs.is_empty() {
        let mut offset = 0;
        visit_range(
            store,
            &head,
            LogRange {
                after: receipt.expected_seq as u64,
                through: receipt.last_seq as u64,
                limit: logs.len() as u16,
            },
            session,
            true,
            |entry| {
                if logs.get(offset) != Some(&entry) {
                    return Err(corrupt());
                }
                offset += 1;
                Ok(())
            },
        )
        .await
        .map_err(error)?;
        if offset != logs.len() {
            return Err(corrupt());
        }
    }
    Ok(Outcome::Recorded(Box::new(receipt)))
}

fn error(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => driver(error),
    }
}

fn driver(_: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable("Agent log publication outcome remains unresolved".into())
}
