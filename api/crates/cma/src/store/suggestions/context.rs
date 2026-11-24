use super::*;
use crate::store::agent_log_integrity::TransactionLogError;
use crate::store::agent_log_segment_reads;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentLogKind, Sandbox};

pub(super) enum ContextError {
    Domain(AgentReadError),
    Driver(mongodb::error::Error),
}

impl ContextError {
    pub(super) fn into_read_error(self) -> AgentReadError {
        match self {
            Self::Domain(error) => error,
            Self::Driver(error) => unavailable(error),
        }
    }
}

impl From<TransactionLogError> for ContextError {
    fn from(error: TransactionLogError) -> Self {
        match error {
            TransactionLogError::Domain(error) => Self::Domain(unavailable(error)),
            TransactionLogError::Driver(error) => Self::Driver(error),
        }
    }
}

async fn bounded_log(
    store: &MongoSandboxStore,
    expected: &Agent,
    after: u64,
    through: u64,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Vec<AgentLogEntry>, TransactionLogError> {
    if through.saturating_sub(after) > MAX_CONTEXT_EVENTS as u64 {
        return Ok(Vec::new());
    }
    if let Some(entries) = agent_log_segment_reads::range_transaction(
        store, expected, session, after, through, snapshot,
    )
    .await?
    {
        return Ok(entries);
    }
    let find = store.agent_log.find(
        doc! {"agent_id":expected.id.as_str(),"seq":{"$gt":after as i64,"$lte":through as i64}},
    );
    let find = if snapshot {
        find.read_concern(mongodb::options::ReadConcern::snapshot())
    } else {
        find
    };
    let mut cursor = find
        .sort(doc! {"seq":1})
        .limit(MAX_CONTEXT_EVENTS as i64)
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let mut entries = Vec::new();
    while cursor
        .advance(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
    {
        entries.push(
            crate::store::agent_log::read_entry(
                &cursor
                    .deserialize_current()
                    .map_err(TransactionLogError::Driver)?,
            )
            .map_err(TransactionLogError::Domain)?,
        );
    }
    Ok(entries)
}

pub(super) async fn current(
    store: &MongoSandboxStore,
    work: &SuggestionWork,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<bool, AgentReadError> {
    current_transaction(store, work, session, snapshot)
        .await
        .map_err(ContextError::into_read_error)
}

pub(super) async fn current_transaction(
    store: &MongoSandboxStore,
    work: &SuggestionWork,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<bool, ContextError> {
    Ok(current_parent(store, work, session, snapshot)
        .await?
        .is_some())
}

pub(super) async fn current_parent(
    store: &MongoSandboxStore,
    work: &SuggestionWork,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Option<Sandbox>, ContextError> {
    let Some(row) = store
        .agents
        .find_one(doc! {"_id":work.agent_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ContextError::Driver)?
    else {
        return Ok(None);
    };
    let agent = crate::store::agents::read_agent(&row)
        .map_err(|error| ContextError::Domain(unavailable(error)))?;
    let Some(row) = store
        .sandboxes
        .find_one(doc! {"_id":agent.sandbox_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(ContextError::Driver)?
    else {
        return Ok(None);
    };
    let sandbox: Sandbox = crate::store::work_schema::read(&row)
        .map_err(|error| ContextError::Domain(unavailable(error)))?;
    let newer = store
        .agent_commands
        .find_one(doc! {"agent_id":work.agent_id.as_str(),
            "record.receipt.event_cursor":{"$gte":work.scope.answer_sequence as i64},
            "record.receipt.envelope.command.type":"message",
        })
        .session(&mut *session)
        .await
        .map_err(ContextError::Driver)?
        .is_some();
    let tail = bounded_log(
        store,
        &agent,
        work.completed_sequence,
        agent.conversation.last_seq,
        session,
        snapshot,
    )
    .await?;
    Ok(work_is_current(work, &agent, &sandbox, &tail, newer).then_some(sandbox))
}

pub(in crate::store) async fn enqueue(
    store: &MongoSandboxStore,
    agent: &Agent,
    logs: &[AgentLogEntry],
    session: &mut ClientSession,
) -> Result<(), TransactionLogError> {
    if !store
        .writer_policy
        .allows_writer(&agent.owner, TaskExperienceWriter::Suggestions)
    {
        return Ok(());
    }
    let Some(completed) = logs
        .iter()
        .find(|entry| matches!(entry.kind, AgentLogKind::TurnCompleted { .. }))
    else {
        return Ok(());
    };
    let AgentLogKind::TurnCompleted { turn_sequence } = completed.kind else {
        return Ok(());
    };
    if turn_sequence == 0
        || completed.seq.saturating_sub(turn_sequence) >= MAX_CONTEXT_EVENTS as u64
    {
        return Ok(());
    }
    let outbox_unavailable = || {
        TransactionLogError::Domain(SandboxStoreError::Unavailable(
            "suggestion outbox unavailable".into(),
        ))
    };
    // Only enqueue receives the exact append batch. An ordinary transaction
    // read/claim/completion never receives permission from `!snapshot`.
    if logs.is_empty()
        || logs
            .last()
            .is_none_or(|entry| entry.seq != agent.conversation.last_seq)
        || logs.iter().any(|entry| entry.agent_id != agent.id)
        || logs
            .windows(2)
            .any(|pair| pair[0].seq.checked_add(1) != Some(pair[1].seq))
    {
        return Err(TransactionLogError::Domain(
            SandboxStoreError::SequenceConflict,
        ));
    }
    let entries = bounded_log(
        store,
        agent,
        turn_sequence - 1,
        completed.seq,
        session,
        false,
    )
    .await
    .map_err(|_| outbox_unavailable())?;
    // The append must already be visible with its exact original identity in
    // this transaction. There is no raw-source fallback after proof refusal.
    if logs
        .iter()
        .filter(|entry| entry.seq >= turn_sequence && entry.seq <= completed.seq)
        .any(|entry| !entries.iter().any(|stored| stored == entry))
    {
        return Err(TransactionLogError::Domain(
            SandboxStoreError::SequenceConflict,
        ));
    }
    let Some(row) = store
        .sandboxes
        .find_one(doc! {"_id":agent.sandbox_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
    else {
        return Ok(());
    };
    let sandbox: Sandbox = crate::store::work_schema::read(&row)
        .map_err(|reason| TransactionLogError::Domain(SandboxStoreError::Corrupt(reason)))?;
    let Some(work) = work_from_turn(agent, &sandbox, &entries) else {
        return Ok(());
    };
    store
        .suggestions
        .update_one(
            doc! {"_id": &work.id},
            doc! {"$setOnInsert": document(&work).map_err(|_| outbox_unavailable())?},
        )
        .upsert(true)
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    Ok(())
}

#[cfg(test)]
mod tests;
