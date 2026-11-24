use super::*;
use layer2_core::sandbox::{
    normalize_reply_preview, AgentReplyPreview, AgentTranscriptRole, AgentTurnOutcome,
    AssistantReplyPreview,
};
use segments::{
    capture::{
        self,
        preview_budget::{PreviewBudget, READ_DEADLINE},
    },
    projection,
};

pub(in crate::store) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
) -> Result<AgentReplyPreview, SandboxStoreError> {
    let mut budget = PreviewBudget::new();
    match tokio::time::timeout(READ_DEADLINE, bounded(store, agent, &mut budget)).await {
        Err(_) | Ok(Err(SandboxStoreError::AgentLogCapacityExceeded)) => {
            Ok(AgentReplyPreview::Unavailable)
        }
        Ok(result) => result,
    }
}

async fn bounded(
    store: &MongoSandboxStore,
    agent: &Agent,
    budget: &mut PreviewBudget,
) -> Result<AgentReplyPreview, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|e| SandboxStoreError::Unavailable(e.to_string()))?;
    let Some(source) = capture::preview(store, agent, &mut session, budget)
        .await
        .map_err(segment_error)?
    else {
        return Ok(AgentReplyPreview::Unavailable);
    };
    let mut through = source.through();
    for _ in 0..4 {
        if through == 0 {
            return Ok(AgentReplyPreview::None {
                source_through_seq: source.through(),
            });
        }
        let first = through.saturating_sub(31).max(1);
        let sequences = (first..=through).collect::<Vec<_>>();
        let rows = capture::preview_points(store, &source, &sequences, &mut session, budget)
            .await
            .map_err(segment_error)?;
        for entry in rows.into_iter().rev() {
            let AgentLogKind::AgentMessage { ref content } = entry.kind else {
                continue;
            };
            let Some((text, truncated)) = normalize_reply_preview(content) else {
                continue;
            };
            let turn_sequence =
                verify_message(store, &source, &entry, &mut session, budget).await?;
            budget.check().map_err(segment_error)?;
            return Ok(AgentReplyPreview::Ready {
                source_through_seq: source.through(),
                reply: AssistantReplyPreview {
                    text,
                    truncated,
                    message_sequence: entry.seq,
                    turn_sequence,
                    appended_at: entry.appended_at,
                },
            });
        }
        through = first - 1;
    }
    Ok(if through == 0 {
        AgentReplyPreview::None {
            source_through_seq: source.through(),
        }
    } else {
        AgentReplyPreview::Unavailable
    })
}

async fn verify_message(
    store: &MongoSandboxStore,
    source: &capture::CapturedLog,
    message: &AgentLogEntry,
    session: &mut ClientSession,
    budget: &mut PreviewBudget,
) -> Result<u64, SandboxStoreError> {
    let head = source.active_head();
    let Some(projection::EvidenceRecord::Message {
        sequence,
        turn_sequence,
        role: AgentTranscriptRole::Agent,
    }) = projection::evidence_budgeted(
        store,
        head,
        "message",
        &message.seq.to_string(),
        session,
        Some(budget),
    )
    .await
    .map_err(segment_error)?
    else {
        return Err(corrupt());
    };
    if sequence != message.seq || turn_sequence >= sequence || message.agent_id != source.agent().id
    {
        return Err(corrupt());
    }
    let Some(projection::EvidenceRecord::Turn { facts }) = projection::evidence_budgeted(
        store,
        head,
        "turn",
        &turn_sequence.to_string(),
        session,
        Some(budget),
    )
    .await
    .map_err(segment_error)?
    else {
        return Err(corrupt());
    };
    facts.validate(source.through()).map_err(|_| corrupt())?;
    if facts.turn_sequence != turn_sequence
        || facts.end_sequence.is_some_and(|end| end <= sequence)
        || facts
            .first_assistant_sequence
            .is_none_or(|first| first > sequence)
    {
        return Err(corrupt());
    }
    let mut sequences = vec![turn_sequence];
    if let Some(end) = facts.end_sequence {
        sequences.push(end);
    }
    let entries = capture::preview_points(store, source, &sequences, session, budget)
        .await
        .map_err(segment_error)?;
    let start = entries.first().ok_or_else(corrupt)?;
    if !matches!(&start.kind, AgentLogKind::TurnStarted { turn_sequence: declared, origin_key, model } if *declared == turn_sequence && origin_key == &facts.origin_key && model == &facts.model)
        || start.appended_at != facts.started_at
    {
        return Err(corrupt());
    }
    let valid_outcome = match (&facts.outcome, entries.get(1)) {
        (AgentTurnOutcome::Active, None) => source
            .projection()
            .active
            .as_ref()
            .is_some_and(|active| active.turn_sequence == turn_sequence),
        (AgentTurnOutcome::Completed { at }, Some(end)) => {
            end.appended_at == *at && end.kind == AgentLogKind::TurnCompleted { turn_sequence }
        }
        (AgentTurnOutcome::Interrupted { at }, Some(end)) => {
            end.appended_at == *at && end.kind == AgentLogKind::TurnInterrupted { turn_sequence }
        }
        (AgentTurnOutcome::Failed { at, failure }, Some(end)) => {
            end.appended_at == *at
                && if facts.source_completed {
                    facts.lost_frames > 0
                        && end.kind == AgentLogKind::TurnCompleted { turn_sequence }
                } else {
                    end.kind
                        == AgentLogKind::TurnFailed {
                            turn_sequence,
                            failure: failure.clone(),
                        }
                }
        }
        _ => false,
    };
    if !valid_outcome {
        return Err(corrupt());
    }
    Ok(turn_sequence)
}

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("reply preview evidence does not match its certified source".into())
}

#[cfg(test)]
mod tests;
