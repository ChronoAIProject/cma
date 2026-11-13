//! A title needs four immutable source points, independent of first-turn length.
use super::*;
use crate::store::agent_log_segments::{capture, projection};
use layer2_core::sandbox::AgentTranscriptRole;

pub(super) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Option<AgentTitleTranscript>, AgentTitleStoreError> {
    let source = capture::capture(store, agent, session, capture::CaptureMode::Snapshot)
        .await
        .map_err(error)?;
    let Some(turn) = source.projection().title_turn else {
        return Ok(None);
    };
    let Some(projection::EvidenceRecord::Turn { facts }) = projection::evidence(
        store,
        source.active_head(),
        "turn",
        &turn.to_string(),
        session,
    )
    .await
    .map_err(error)?
    else {
        return Err(AgentTitleStoreError::Corrupt);
    };
    if facts.turn_sequence != turn {
        return Err(AgentTitleStoreError::Corrupt);
    }
    if !facts.title_eligible || !facts.source_completed {
        return Ok(None);
    }
    let (Some(owner), Some((answer, AgentTranscriptRole::Agent, _)), Some(end)) = (
        facts.owner_sequence,
        facts.last_message.as_ref(),
        facts.end_sequence,
    ) else {
        return Ok(None);
    };
    let rows = capture::points(
        store,
        &source,
        &[turn, owner, *answer, end],
        session,
        capture::CaptureMode::Snapshot,
    )
    .await
    .map_err(error)?;
    if !matches!(&rows[0].kind, AgentLogKind::TurnStarted {turn_sequence, origin_key, model}
        if *turn_sequence == turn && origin_key == &facts.origin_key && model == &facts.model)
        || rows[0].appended_at != facts.started_at
        || !matches!(rows[1].kind, AgentLogKind::OwnerMessage { .. })
        || !matches!(rows[2].kind, AgentLogKind::AgentMessage { .. })
        || !matches!(rows[3].kind, AgentLogKind::TurnCompleted { turn_sequence } if turn_sequence == turn)
    {
        return Err(AgentTitleStoreError::Corrupt);
    }
    // Eligibility is certified by the exact writer fold. The original title
    // fold still owns truncation and the selected owner/last-answer semantics.
    Ok(AgentTitleTranscript::from_complete_log(&rows))
}

fn error(error: crate::store::agent_log_integrity::TransactionLogError) -> AgentTitleStoreError {
    match error {
        crate::store::agent_log_integrity::TransactionLogError::Domain(error) => {
            map_store_error(error)
        }
        crate::store::agent_log_integrity::TransactionLogError::Driver(error) => unavailable(error),
    }
}
