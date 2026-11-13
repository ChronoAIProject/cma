//! Exact final-answer evidence; work is independent of the turn's length.
use super::*;
use layer2_core::sandbox::{
    store::AgentLogFinalAnswer, AgentTranscriptMessage, AgentTranscriptRole, AgentTurnOutcome,
};
use segments::{capture, projection};

pub(in crate::store) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    sequence: u64,
    session: &mut ClientSession,
) -> Result<Option<AgentLogFinalAnswer>, SandboxStoreError> {
    if segmented_source(store, agent, session, true)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    let source = capture::capture(store, agent, session, capture::CaptureMode::Snapshot)
        .await
        .map_err(segment_error)?;
    if sequence == 0 || sequence > source.through() {
        return Err(SandboxStoreError::NotFound);
    }
    let message = capture::points(
        store,
        &source,
        &[sequence],
        session,
        capture::CaptureMode::Snapshot,
    )
    .await
    .map_err(segment_error)?
    .pop()
    .ok_or_else(corrupt)?;
    let AgentLogKind::AgentMessage { content } = message.kind else {
        return Err(SandboxStoreError::NotFound);
    };
    let Some(projection::EvidenceRecord::Message {
        sequence: recorded,
        turn_sequence,
        role: AgentTranscriptRole::Agent,
    }) = projection::evidence(
        store,
        source.active_head(),
        "message",
        &sequence.to_string(),
        session,
    )
    .await
    .map_err(segment_error)?
    else {
        return Err(corrupt());
    };
    if recorded != sequence || turn_sequence >= sequence {
        return Err(corrupt());
    }
    let Some(projection::EvidenceRecord::Turn { facts }) = projection::evidence(
        store,
        source.active_head(),
        "turn",
        &turn_sequence.to_string(),
        session,
    )
    .await
    .map_err(segment_error)?
    else {
        return Err(corrupt());
    };
    facts.validate(source.through()).map_err(|_| corrupt())?;
    if facts.turn_sequence != turn_sequence {
        return Err(corrupt());
    }
    if facts.final_answer_sequence() != Some(sequence) {
        return Err(SandboxStoreError::NotFound);
    }
    let end = facts.end_sequence.ok_or_else(corrupt)?;
    let rows = capture::points(
        store,
        &source,
        &[turn_sequence, end],
        session,
        capture::CaptureMode::Snapshot,
    )
    .await
    .map_err(segment_error)?;
    let [start, terminal] = rows.as_slice() else {
        return Err(corrupt());
    };
    if !matches!(&start.kind, AgentLogKind::TurnStarted { turn_sequence: declared, origin_key, model }
        if *declared == turn_sequence && origin_key == &facts.origin_key && model == &facts.model)
        || start.appended_at != facts.started_at
        || terminal.kind != (AgentLogKind::TurnCompleted { turn_sequence })
        || facts.outcome
            != (AgentTurnOutcome::Completed {
                at: terminal.appended_at.clone(),
            })
        || content.trim().is_empty()
    {
        return Err(corrupt());
    }
    Ok(Some(AgentLogFinalAnswer {
        turn_sequence,
        message: AgentTranscriptMessage {
            sequence,
            role: AgentTranscriptRole::Agent,
            content,
            appended_at: message.appended_at,
        },
    }))
}

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("final answer evidence does not match its certified source".into())
}
