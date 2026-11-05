//! Bounded original-event proof for publication reconciliation.
//!
//! Aggregate/effect receipts remain the caller's responsibility. These checks
//! prove only the exact source batch and never infer success from a later head.
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};

use super::{
    agent_log::scan_complete_log_with_snapshot_session,
    agent_log_integrity::{ensure_append_batch_size, TransactionLogError},
    agent_log_segments::{read_format, read_head, visit_range, LogRange},
    agents::read_agent,
    sandbox_mongo::MongoSandboxStore,
};

pub(super) async fn matches(
    store: &MongoSandboxStore,
    agent: &Agent,
    expected: &[AgentLogEntry],
    session: &mut ClientSession,
) -> Result<bool, SandboxStoreError> {
    ensure_append_batch_size(expected.len())?;
    let Some(first) = expected.first() else {
        // A publication without source events still requires a valid captured
        // head, but does not need an unbounded historical scan.
        super::agent_log_context::validate(store, agent, session, true)
            .await
            .map_err(error)?;
        return Ok(true);
    };
    if first.seq == 0
        || expected.iter().enumerate().any(|(offset, entry)| {
            entry.agent_id != agent.id
                || first.seq.checked_add(offset as u64) != Some(entry.seq)
                || entry.seq > agent.conversation.last_seq
        })
    {
        return Ok(false);
    }
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .read_concern(ReadConcern::snapshot())
        .session(&mut *session)
        .await
        .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?
        .ok_or(SandboxStoreError::NotFound)?;
    if read_agent(&row)? != *agent {
        return Err(SandboxStoreError::Conflict);
    }
    let last = expected.last().expect("nonempty source batch").seq;
    let mut offset = 0;
    let mut compare = |entry: AgentLogEntry| {
        if entry.seq >= first.seq && entry.seq <= last {
            if expected.get(offset) != Some(&entry) {
                return Err(SandboxStoreError::SequenceConflict);
            }
            offset += 1;
        }
        Ok(())
    };
    if read_format(&row)?.is_some() {
        let head = read_head(store, &row, session, true).await.map_err(error)?;
        visit_range(
            store,
            &head,
            LogRange {
                after: first.seq - 1,
                through: last,
                limit: expected.len() as u16,
            },
            session,
            true,
            &mut compare,
        )
        .await
        .map_err(error)?;
    } else {
        scan_complete_log_with_snapshot_session(
            store,
            &agent.id,
            agent.conversation.last_seq,
            session,
            &mut compare,
        )
        .await
        .map_err(error)?;
    }
    Ok(offset == expected.len())
}

fn error(error: TransactionLogError) -> SandboxStoreError {
    match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
    }
}
