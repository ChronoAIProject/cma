use super::source::bounded_log;
use super::*;
use crate::store::agent_log_integrity::TransactionLogError;
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentLogKind, Sandbox};

pub(super) async fn current(
    store: &MongoSandboxStore,
    work: &SuggestionWork,
    session: &mut ClientSession,
) -> Result<bool, AgentReadError> {
    let Some(row) = store
        .agents
        .find_one(doc! {"_id":work.agent_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
    else {
        return Ok(false);
    };
    let agent = crate::store::agents::read_agent(&row).map_err(unavailable)?;
    let Some(row) = store
        .sandboxes
        .find_one(doc! {"_id":agent.sandbox_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(unavailable)?
    else {
        return Ok(false);
    };
    let sandbox: Sandbox = read_record(&row).map_err(unavailable)?;
    let newer = store
        .agent_commands
        .find_one(doc! {"agent_id":work.agent_id.as_str(),
            "record.receipt.event_cursor":{"$gte":work.scope.answer_sequence as i64},
            "record.receipt.envelope.command.type":"message",
        })
        .session(&mut *session)
        .await
        .map_err(unavailable)?
        .is_some();
    let tail = bounded_log(
        store,
        &agent,
        work.completed_sequence,
        agent.conversation.last_seq,
        session,
    )
    .await
    .map_err(unavailable)?;
    Ok(work_is_current(work, &agent, &sandbox, &tail, newer))
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
    let map_error = |_| {
        TransactionLogError::Domain(SandboxStoreError::Unavailable(
            "suggestion outbox unavailable".into(),
        ))
    };
    let entries = bounded_log(store, agent, turn_sequence - 1, completed.seq, session).await?;
    let Some(row) = store
        .sandboxes
        .find_one(doc! {"_id":agent.sandbox_id.as_str()})
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
    else {
        return Ok(());
    };
    let sandbox: Sandbox = read_record(&row)
        .map_err(|reason| TransactionLogError::Domain(SandboxStoreError::Corrupt(reason)))?;
    let Some(work) = work_from_turn(agent, &sandbox, &entries) else {
        return Ok(());
    };
    store
        .suggestions
        .update_one(
            doc! {"_id": &work.id},
            doc! {"$setOnInsert": document(&work).map_err(map_error)?},
        )
        .upsert(true)
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    Ok(())
}
