//! Verify optional suggestion sources while retaining only a small context.
use super::*;
use crate::store::{
    agent_log::read_entry, agent_log_integrity::TransactionLogError,
    agent_log_segments as segments, agents::read_agent,
};
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry};

pub(super) async fn bounded_log(
    store: &MongoSandboxStore,
    agent: &Agent,
    after: u64,
    through: u64,
    session: &mut ClientSession,
) -> Result<Vec<AgentLogEntry>, TransactionLogError> {
    let count = through
        .checked_sub(after)
        .filter(|_| through <= agent.conversation.last_seq)
        .ok_or(SandboxStoreError::SequenceConflict)?;
    if count > MAX_CONTEXT_EVENTS as u64 {
        return Ok(Vec::new());
    }
    let row = store
        .agents
        .find_one(doc! {"_id":agent.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or(SandboxStoreError::NotFound)?;
    if read_agent(&row)? != *agent {
        return Err(SandboxStoreError::Conflict.into());
    }
    let mut entries = Vec::new();
    let mut remaining = Some(MAX_CONTEXT_BYTES);
    let mut expected = after;
    let mut visit = |entry: AgentLogEntry| {
        expected = expected
            .checked_add(1)
            .ok_or(SandboxStoreError::SequenceConflict)?;
        if entry.agent_id != agent.id || entry.seq != expected || entry.seq > through {
            return Err(SandboxStoreError::SequenceConflict);
        }
        remaining = remaining.and_then(|left| left.checked_sub(context_entry_bytes(&entry)?));
        if remaining.is_some() {
            entries.push(entry);
        } else {
            // An oversized optional context is skipped, while the source
            // reader continues to verify the complete requested interval.
            entries.clear();
        }
        Ok(())
    };
    if segments::read_format(&row)?.is_some() {
        let head = segments::read_head(store, &row, session, false).await?;
        segments::visit_range(
            store,
            &head,
            segments::LogRange {
                after,
                through,
                limit: MAX_CONTEXT_EVENTS as u16,
            },
            session,
            false,
            &mut visit,
        )
        .await?;
    } else {
        let mut cursor = store
            .agent_log
            .find(doc! {
                "agent_id":agent.id.as_str(),
                "seq":{
                    "$gt":i64::try_from(after).map_err(|_|SandboxStoreError::SequenceConflict)?,
                    "$lte":i64::try_from(through).map_err(|_|SandboxStoreError::SequenceConflict)?,
                },
            })
            .sort(doc! {"seq":1})
            .limit(MAX_CONTEXT_EVENTS as i64)
            .batch_size(1)
            .session(&mut *session)
            .await
            .map_err(TransactionLogError::Driver)?;
        while cursor
            .advance(&mut *session)
            .await
            .map_err(TransactionLogError::Driver)?
        {
            let row = cursor
                .deserialize_current()
                .map_err(TransactionLogError::Driver)?;
            visit(read_entry(&row)?)?;
        }
    }
    if expected != through {
        return Err(SandboxStoreError::SequenceConflict.into());
    }
    Ok(entries)
}
