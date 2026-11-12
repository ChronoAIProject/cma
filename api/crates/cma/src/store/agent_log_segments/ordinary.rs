//! Transaction-local proof for ordinary admission after gated v2 adoption.
//! Read only bounded active state and indexed original evidence, never lifetime text.
use super::{
    corrupt,
    projection::{self, EvidenceRecord},
    read_format, read_head, visit_range, Head, LogRange,
};
use crate::store::{
    agent_log_integrity::TransactionLogError, agents::read_agent, sandbox_mongo::MongoSandboxStore,
};
use layer2_core::sandbox::{
    agent_io::AnswerContext, Agent, AgentLogEntry, AgentLogKind, AgentLogProjectionState,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

type Result<T> = std::result::Result<T, TransactionLogError>;

pub(in crate::store) async fn source(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut ClientSession,
) -> Result<Option<(Head, AgentLogProjectionState)>> {
    if read_agent(row)? != *agent {
        return Err(corrupt().into());
    }
    if read_format(row)?.is_none() {
        return Ok(None);
    }
    let head = read_head(store, row, session, false).await?;
    let state = projection::read(store, agent, &head, session).await?;
    // An empty requested interval still certifies the bounded active tail and
    // its predecessor seal, and rejects unpublished rows beyond the head.
    visit_range(
        store,
        &head,
        LogRange {
            after: head.last_seq as u64,
            through: head.last_seq as u64,
            limit: 1,
        },
        session,
        false,
        |_| Ok(()),
    )
    .await?;
    Ok(Some((head, state)))
}

async fn point(
    store: &MongoSandboxStore,
    head: &Head,
    sequence: u64,
    session: &mut ClientSession,
) -> Result<AgentLogEntry> {
    if sequence == 0 || sequence > head.last_seq as u64 {
        return Err(corrupt().into());
    }
    let mut found = None;
    visit_range(
        store,
        head,
        LogRange {
            after: sequence - 1,
            through: sequence,
            limit: 1,
        },
        session,
        false,
        |entry| {
            found = Some(entry);
            Ok(())
        },
    )
    .await?;
    found.ok_or_else(|| corrupt().into())
}

pub(in crate::store) async fn answer(
    store: &MongoSandboxStore,
    agent: &Agent,
    row: &Document,
    session: &mut ClientSession,
) -> Result<Option<Option<AnswerContext>>> {
    let Some((head, state)) = source(store, agent, row, session).await? else {
        return Ok(None);
    };
    let Some(turn) = state.latest_turn else {
        return Ok(Some(None));
    };
    let Some(EvidenceRecord::Turn { facts }) =
        projection::evidence(store, &head, "turn", &turn.to_string(), session).await?
    else {
        return Err(corrupt().into());
    };
    let expected = state
        .active
        .as_ref()
        .into_iter()
        .chain(state.latest_completed.as_ref())
        .find(|facts| facts.turn_sequence == turn);
    if facts.turn_sequence != turn || expected != Some(facts.as_ref()) {
        return Err(corrupt().into());
    }
    let Some(sequence) = facts.final_answer_sequence() else {
        return Ok(Some(None));
    };
    let start = point(store, &head, turn, session).await?;
    let message = point(store, &head, sequence, session).await?;
    let end = point(
        store,
        &head,
        facts.end_sequence.ok_or_else(corrupt)?,
        session,
    )
    .await?;
    if !matches!(start.kind, AgentLogKind::TurnStarted { turn_sequence, .. } if turn_sequence == turn)
        || !matches!(message.kind, AgentLogKind::AgentMessage { ref content } if !content.trim().is_empty())
        || end.kind
            != (AgentLogKind::TurnCompleted {
                turn_sequence: turn,
            })
    {
        return Err(corrupt().into());
    }
    Ok(Some(Some(AnswerContext {
        turn_sequence: turn,
        answer_sequence: sequence,
    })))
}

pub(in crate::store) async fn approval(
    store: &MongoSandboxStore,
    agent: &Agent,
    approval_id: &str,
    session: &mut ClientSession,
) -> Result<Option<Option<AgentLogEntry>>> {
    let row = store
        .agents
        .find_one(doc! {"_id": agent.id.as_str()})
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(corrupt)?;
    let Some((head, state)) = source(store, agent, &row, session).await? else {
        return Ok(None);
    };
    let Some(EvidenceRecord::Approval {
        approval_id: stored,
        request_sequence,
        turn_sequence,
        epoch,
    }) = projection::evidence(store, &head, "approval", approval_id, session).await?
    else {
        return Ok(Some(None));
    };
    if stored != approval_id || epoch > state.approval_epoch {
        return Err(corrupt().into());
    }
    if epoch < state.approval_epoch || request_sequence.is_none() {
        return Ok(Some(None));
    }
    if turn_sequence != agent.conversation.active_turn {
        return Err(corrupt().into());
    }
    let entry = point(store, &head, request_sequence.ok_or_else(corrupt)?, session).await?;
    if !matches!(&entry.kind, AgentLogKind::ApprovalRequested { approval_id: stored, .. } if stored == approval_id)
    {
        return Err(corrupt().into());
    }
    Ok(Some(Some(entry)))
}
