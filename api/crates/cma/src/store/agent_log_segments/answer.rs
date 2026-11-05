use layer2_core::sandbox::{agent_io::AnswerContext, Agent, AgentLogKind};
use mongodb::ClientSession;

use super::{
    consumers::{points, semantic},
    corrupt,
    projection::{self, EvidenceRecord},
};
use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::MongoSandboxStore};

/// The outer None selects legacy validation; a certified segmented history
/// without a final answer returns Some(None). Neither case permits redispatch.
pub(in crate::store) async fn context(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Option<Option<AnswerContext>>, TransactionLogError> {
    let Some((head, state)) = semantic(store, agent, session, false).await? else {
        return Ok(None);
    };
    let Some(turn_sequence) = state.latest_turn else {
        return Ok(Some(None));
    };
    let Some(EvidenceRecord::Turn { facts }) =
        projection::evidence(store, &head, "turn", &turn_sequence.to_string(), session).await?
    else {
        return Err(corrupt().into());
    };
    if facts.turn_sequence != turn_sequence {
        return Err(corrupt().into());
    }
    let Some(answer_sequence) = facts.final_answer_sequence() else {
        return Ok(Some(None));
    };
    let end = facts.end_sequence.ok_or_else(corrupt)?;
    if turn_sequence >= answer_sequence || answer_sequence >= end {
        return Err(corrupt().into());
    }
    let entries = points(
        store,
        &head,
        &[turn_sequence, answer_sequence, end],
        session,
        false,
    )
    .await?;
    if !matches!(entries[0].kind, AgentLogKind::TurnStarted { turn_sequence: declared, .. } if declared == turn_sequence)
        || !matches!(&entries[1].kind, AgentLogKind::AgentMessage { content } if !content.trim().is_empty())
        || entries[2].kind != (AgentLogKind::TurnCompleted { turn_sequence })
    {
        return Err(corrupt().into());
    }
    Ok(Some(Some(AnswerContext {
        turn_sequence,
        answer_sequence,
    })))
}
