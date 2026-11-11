//! Snapshot-preserving proof adapter for bounded public log readers.
use super::*;
use mongodb::options::ReadConcern;

#[derive(Debug, PartialEq, Eq)]
#[cfg(test)]
pub(in crate::store) struct ReadHead {
    pub(in crate::store) last_seq: u64,
    pub(in crate::store) trusted: bool,
}

/// Use the supplied snapshot for every query. Absence is distinct from legacy
/// or disabled evidence. Corruption remains an error but never initiates repair,
/// quarantine, an audit, or a transaction operation from a public read.
#[cfg(test)]
pub(in crate::store) async fn read_head(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    session: &mut mongodb::ClientSession,
) -> Result<Option<ReadHead>, SandboxStoreError> {
    Ok(read_proven_agent(store, agent_id, session)
        .await?
        .map(|(agent, trusted)| ReadHead {
            last_seq: agent.conversation.last_seq,
            trusted,
        }))
}

/// Retain the validated Agent for a caller's read-only complete fallback. This
/// avoids rereading the envelope or invoking the writer/audit quarantine path.
pub(in crate::store) async fn read_proven_agent(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    session: &mut mongodb::ClientSession,
) -> Result<Option<(Agent, bool)>, SandboxStoreError> {
    let row = store
        .agents
        .find_one(doc! { "_id": agent_id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let agent = crate::store::agents::read_agent(&row)?;
    if crate::store::agent_log_segments::read_format(&row)?.is_some() {
        return Err(SandboxStoreError::Unavailable(
            "segmented history requires a bounded reader".into(),
        ));
    }
    let trusted = legacy_proof(store, &agent, &row, session).await?;
    Ok(Some((agent, trusted)))
}

pub(in crate::store) async fn legacy_proof(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut mongodb::ClientSession,
) -> Result<bool, SandboxStoreError> {
    legacy_proof_for_context(store, agent, row, session, true).await
}

pub(in crate::store) async fn legacy_proof_for_context(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut mongodb::ClientSession,
    snapshot: bool,
) -> Result<bool, SandboxStoreError> {
    let agent_id = &agent.id;
    if store.bounded_log_enabled(agent_id) && row.contains_key(QUARANTINE_FIELD) {
        fleet::telemetry::record(fleet::telemetry::Event::Quarantined);
    }
    let proof = proof::read_only_summary(store, agent, row, session, snapshot)
        .await
        .map_err(|error| match error {
            TransactionLogError::Domain(error) => error,
            TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
        })?;
    let trusted = proof.is_some() && store.bounded_log_enabled(agent_id);
    Ok(trusted)
}
