//! Snapshot-preserving proof adapter for bounded public log readers.
use super::*;
use mongodb::options::ReadConcern;

#[derive(Debug, PartialEq, Eq)]
pub(in crate::store) struct ReadHead {
    pub(in crate::store) last_seq: u64,
    pub(in crate::store) trusted: bool,
}

/// Use the supplied snapshot for every query. Absence is distinct from legacy
/// or disabled evidence. Corruption remains an error but never initiates repair,
/// quarantine, an audit, or a transaction operation from a public read.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "consumer lands in the dependent #1320 public-window slice"
    )
)]
pub(in crate::store) async fn read_head(
    store: &MongoSandboxStore,
    agent_id: &AgentId,
    session: &mut mongodb::ClientSession,
) -> Result<Option<ReadHead>, SandboxStoreError> {
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
    let proof = proof::read_only_summary(store, &agent, &row, session, true)
        .await
        .map_err(|error| match error {
            TransactionLogError::Domain(error) => error,
            TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
        })?;
    Ok(Some(ReadHead {
        last_seq: agent.conversation.last_seq,
        trusted: proof.is_some() && store.bounded_admission_agents.contains(agent_id),
    }))
}
