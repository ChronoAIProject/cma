//! Exact receipt reconciliation in one majority-visible snapshot. A read can
//! prove Recorded, never KnownAbort. Missing/unavailable evidence stays Unknown.
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry};
use mongodb::{
    bson::{doc, Binary},
    options::ReadConcern,
    ClientSession,
};

use super::{
    collections::collection,
    corrupt, digest,
    publication::{self, Publication, PublicationIdentity},
    read_format, read_head, visit_range, LogRange, Seal, PUBLICATIONS, SEGMENTS,
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

#[path = "writer_keyed_replay.rs"]
mod keyed;
pub(in crate::store) use keyed::replay;

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
    let mut phase = super::telemetry::Phase::new("reconcile", false);
    let result = Box::pin(reconcile_mode(
        store, operation, expected, logs, result, session, true,
    ))
    .await;
    phase.reconciliation(&result);
    result
}

pub(in crate::store) async fn reconcile_transaction(
    store: &MongoSandboxStore,
    operation: &PublicationIdentity,
    expected: &Agent,
    logs: &[AgentLogEntry],
    result: &Binary,
    session: &mut ClientSession,
) -> Result<Outcome, SandboxStoreError> {
    Box::pin(reconcile_mode(
        store, operation, expected, logs, result, session, false,
    ))
    .await
}

pub(in crate::store) async fn reconcile_scoped(
    store: &MongoSandboxStore,
    operation: &PublicationIdentity,
    expected: &Agent,
    logs: &[AgentLogEntry],
    result: &Binary,
    scope: &mut super::transaction_scope::TransactionScope<'_>,
) -> Result<Outcome, SandboxStoreError> {
    reconcile_transaction(store, operation, expected, logs, result, scope.session()).await
}

async fn reconcile_mode(
    store: &MongoSandboxStore,
    operation: &PublicationIdentity,
    expected: &Agent,
    logs: &[AgentLogEntry],
    result: &Binary,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Outcome, SandboxStoreError> {
    operation.validate()?;
    ensure_append_batch_size(logs.len())?;
    let incarnation = digest::incarnation(expected);
    let publications = collection(store, PUBLICATIONS);
    let query = publications.find_one(doc! {
        "_id":publication::identity(expected.id.as_str(), &incarnation, &operation.key),
    });
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let receipt = query.session(&mut *session).await.map_err(driver)?;
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
    let query = store.agents.find_one(doc! {"_id":expected.id.as_str()});
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let row = query
        .session(&mut *session)
        .await
        .map_err(driver)?
        .ok_or_else(corrupt)?;
    if read_format(&row)?.is_none() {
        return Err(corrupt());
    }
    let current = read_agent(&row)?;
    let head = read_head(store, &row, session, snapshot)
        .await
        .map_err(error)?;
    verify_head(store, &receipt, &current, &head, session, snapshot).await?;
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
            snapshot,
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

/// Verify a publication retained by an immutable domain receipt. The domain
/// adapter validates its request identity and expected event kinds separately.
pub(in crate::store) async fn retained(
    store: &MongoSandboxStore,
    expected: &Publication,
    current: &Agent,
    row: &mongodb::bson::Document,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Vec<AgentLogEntry>, TransactionLogError> {
    expected.validate()?;
    if read_format(row)?.is_none()
        || read_agent(row)? != *current
        || expected.agent_id != current.id.as_str()
        || expected.incarnation != digest::incarnation(current)
    {
        return Err(corrupt().into());
    }
    let receipts = collection(store, PUBLICATIONS);
    let query = receipts.find_one(doc! {"_id":&expected.id});
    let query = if snapshot {
        query.read_concern(ReadConcern::snapshot())
    } else {
        query
    };
    let actual = query
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(corrupt)?;
    if Publication::read(actual)? != *expected {
        return Err(corrupt().into());
    }
    let head = read_head(store, row, session, snapshot).await?;
    verify_head(store, expected, current, &head, session, snapshot).await?;
    let count = u16::try_from(expected.last_seq - expected.expected_seq).map_err(|_| corrupt())?;
    let mut entries = Vec::with_capacity(usize::from(count));
    visit_range(
        store,
        &head,
        LogRange {
            after: expected.expected_seq as u64,
            through: expected.last_seq as u64,
            limit: count.max(1),
        },
        session,
        snapshot,
        |entry| {
            if entry.appended_at != expected.appended_at {
                return Err(corrupt());
            }
            entries.push(entry);
            Ok(())
        },
    )
    .await?;
    if entries.len() != usize::from(count) {
        return Err(corrupt().into());
    }
    Ok(entries)
}

async fn verify_head(
    store: &MongoSandboxStore,
    receipt: &Publication,
    current: &Agent,
    head: &super::Head,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<(), SandboxStoreError> {
    if head.incarnation != receipt.incarnation
        || current.revision < receipt.result_revision as u64
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
        let seals = collection(store, SEGMENTS);
        let query =
            seals.find_one(doc! {"_id":Seal::identity(&head.agent_id, &head.incarnation, ordinal)});
        let query = if snapshot {
            query.read_concern(ReadConcern::snapshot())
        } else {
            query
        };
        let seal = query
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
    Ok(())
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
