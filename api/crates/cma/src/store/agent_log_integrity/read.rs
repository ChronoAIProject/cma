//! Read-only integrity proof for public log readers and complete assessments.
use super::*;
use mongodb::options::ReadConcern;

/// Complete assessment inside the caller's transaction. Retain the shared
/// quarantine/metadata/tail proof and creation lineage without audit side effects.
pub(in crate::store) async fn scan_complete_log_with_session(
    caller: ScanCaller,
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut mongodb::ClientSession,
) -> Result<LogSummary, TransactionLogError> {
    let row = store
        .agents
        .find_one(doc! { "_id": agent.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    if crate::store::agents::read_agent(&row)? != *agent {
        return Err(SandboxStoreError::Conflict.into());
    }
    proof::read_only_summary(store, agent, &row, session, false).await?;
    scan::scan_collection(
        caller,
        &store.agent_log,
        &agent.id,
        agent.conversation.last_seq,
        session,
        false,
        |entry| {
            if let layer2_core::sandbox::AgentLogKind::Created {
                workspace_id,
                worktree_id,
            } = &entry.kind
            {
                if entry.seq != 1
                    || workspace_id != &agent.workspace_id
                    || worktree_id != agent.creation_worktree_id()
                {
                    return Err(SandboxStoreError::Corrupt(
                        "Agent creation log lineage does not match".into(),
                    ));
                }
            }
            Ok(())
        },
    )
    .await
}

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
    if store.bounded_log_enabled(agent_id) && row.contains_key(QUARANTINE_FIELD) {
        fleet::telemetry::record(fleet::telemetry::Event::Quarantined);
    }
    let proof = proof::read_only_summary(store, &agent, &row, session, true)
        .await
        .map_err(|error| match error {
            TransactionLogError::Domain(error) => error,
            TransactionLogError::Driver(error) => SandboxStoreError::Unavailable(error.to_string()),
        })?;
    let trusted = proof.is_some() && store.bounded_log_enabled(agent_id);
    Ok(Some((agent, trusted)))
}
