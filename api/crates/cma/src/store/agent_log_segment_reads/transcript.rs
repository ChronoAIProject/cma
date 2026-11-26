//! Bounded turn seeks and source pages. No request walks from sequence zero.
use super::*;
use layer2_core::sandbox::{
    AgentLogTurnFacts, AgentTranscriptMessage, AgentTranscriptRole, AgentTranscriptTurn,
};
use mongodb::options::Hint;
use segments::{capture, projection};

mod materialize;
#[cfg(test)]
mod tests;

const MAX_PAGE: u16 = 32;
const MAX_COMPATIBILITY_ROWS: u64 = 200;
const MAX_BYTES: usize = 4 * 1024 * 1024;

pub(super) async fn complete(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Option<AgentTranscript>, SandboxStoreError> {
    if !is_segmented(store, agent, session).await? {
        return Ok(None);
    }
    let source = capture::capture(store, agent, session, capture::CaptureMode::Snapshot)
        .await
        .map_err(segment_error)?;
    let mut result = AgentTranscript {
        agent_id: agent.id.clone(),
        through_seq: source.through(),
        turns: vec![],
    };
    let mut turn = next_turn(store, &source, session, 0).await?;
    let mut rows = 0;
    let mut bytes = serde_json::to_vec(&result).map_err(|_| corrupt())?.len();
    while let Some(facts) = turn {
        let end = facts.end_sequence.unwrap_or(source.through());
        rows += end - facts.turn_sequence + 1;
        if rows > MAX_COMPATIBILITY_ROWS {
            return Err(SandboxStoreError::InvalidPage);
        }
        let mut after = facts.turn_sequence - 1;
        let mut value = materialize::header(store, &source, session, &facts).await?;
        let separator = usize::from(!result.turns.is_empty());
        let available = MAX_BYTES
            .checked_sub(bytes + separator)
            .ok_or(SandboxStoreError::AgentLogCapacityExceeded)?;
        if serde_json::to_vec(&value).map_err(|_| corrupt())?.len() > available {
            return Err(SandboxStoreError::AgentLogCapacityExceeded);
        }
        while after < end {
            let through = end.min(after + u64::from(MAX_PAGE));
            let entries = points(
                store,
                &source,
                session,
                &(after + 1..=through).collect::<Vec<_>>(),
            )
            .await?;
            let count = entries.len();
            if materialize::append(&facts, &mut value, entries, available)? != count {
                return Err(SandboxStoreError::AgentLogCapacityExceeded);
            }
            after = through;
        }
        bytes += serde_json::to_vec(&value).map_err(|_| corrupt())?.len() + separator;
        if bytes > MAX_BYTES {
            return Err(SandboxStoreError::InvalidPage);
        }
        let sequence = facts.turn_sequence;
        result.turns.push(value);
        turn = next_turn(store, &source, session, sequence).await?;
    }
    Ok(Some(result))
}

/// Cursor is (turn sequence, last source sequence). A fixed high-water mark is
/// required on continuations; changed snapshots are refused, never mixed.
pub(super) async fn page_with_capture(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    request: &layer2_core::sandbox::AgentTranscriptPageRequest,
    capture_mode: capture::CaptureMode,
) -> Result<Option<layer2_core::sandbox::AgentTranscriptPage>, SandboxStoreError> {
    request
        .validate()
        .map_err(|_| SandboxStoreError::InvalidPage)?;
    if request
        .through
        .is_some_and(|seq| seq != agent.conversation.last_seq)
    {
        return Err(SandboxStoreError::SequenceConflict);
    }
    if !is_segmented(store, agent, session).await? {
        return crate::store::agent_opening_reads::transcript::page(store, agent, session, request)
            .await;
    }
    let source = capture::capture(store, agent, session, capture_mode)
        .await
        .map_err(segment_error)?;
    let facts = if let Some(turn) = request.turn {
        Some(turn_at(store, &source, session, turn).await?)
    } else {
        next_turn(store, &source, session, 0).await?
    };
    let mut transcript = AgentTranscript {
        agent_id: agent.id.clone(),
        through_seq: source.through(),
        turns: vec![],
    };
    let Some(facts) = facts else {
        return Ok(Some(layer2_core::sandbox::AgentTranscriptPage {
            transcript,
            next: None,
        }));
    };
    let after = request.after.unwrap_or(facts.turn_sequence - 1);
    let end = facts.end_sequence.unwrap_or(source.through());
    if after < facts.turn_sequence - 1 || after >= end {
        return Err(SandboxStoreError::InvalidPage);
    }
    let through = end.min(after + u64::from(request.limit));
    let mut value = materialize::header(store, &source, session, &facts).await?;
    if after != facts.turn_sequence - 1 {
        value.messages.clear();
    }
    let entries = super::page::points(store, &source, session, after, through).await?;
    let admitted = materialize::append(&facts, &mut value, entries, MAX_BYTES)?;
    if admitted == 0 {
        return Err(SandboxStoreError::AgentLogCapacityExceeded);
    }
    let through = after + admitted as u64;
    let next = if through < end {
        Some((facts.turn_sequence, through))
    } else {
        next_turn(store, &source, session, facts.turn_sequence)
            .await?
            .map(|next| (next.turn_sequence, next.turn_sequence - 1))
    }
    .map(
        |(turn, after)| layer2_core::sandbox::AgentTranscriptPageRequest {
            turn: Some(turn),
            after: Some(after),
            through: Some(source.through()),
            limit: request.limit,
        },
    );
    transcript.turns.push(value);
    Ok(Some(layer2_core::sandbox::AgentTranscriptPage {
        transcript,
        next,
    }))
}

#[cfg(test)]
async fn page(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    request: &layer2_core::sandbox::AgentTranscriptPageRequest,
) -> Result<Option<layer2_core::sandbox::AgentTranscriptPage>, SandboxStoreError> {
    page_with_capture(
        store,
        agent,
        session,
        request,
        capture::CaptureMode::Snapshot,
    )
    .await
}

async fn next_turn(
    store: &MongoSandboxStore,
    source: &capture::CapturedLog,
    session: &mut ClientSession,
    after: u64,
) -> Result<Option<AgentLogTurnFacts>, SandboxStoreError> {
    let head = source.active_head();
    let row = store
        .segment_logs
        .evidence
        .find_one(doc! {
            "agent_id":&head.agent_id,"incarnation":&head.incarnation,
            "evidence_generation":&head.evidence_generation,"kind":"turn",
            "record.facts.turn_sequence":{"$gt":after as i64},
        })
        .sort(doc! {"record.facts.turn_sequence":1,"entity_key":1})
        .hint(Hint::Name("agent_log_evidence_turn".into()))
        .max_time(std::time::Duration::from_secs(2))
        .read_concern(ReadConcern::snapshot())
        .session(session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let Some(row) = row else {
        if source
            .projection()
            .latest_turn
            .is_some_and(|seq| seq > after)
        {
            return Err(corrupt());
        }
        return Ok(None);
    };
    let key = row.get_str("entity_key").map_err(|_| corrupt())?.to_owned();
    let projection::EvidenceRecord::Turn { facts } =
        projection::read_evidence(head, "turn", &key, row)?
    else {
        return Err(corrupt());
    };
    if facts.turn_sequence <= after || facts.previous_turn != (after != 0).then_some(after) {
        return Err(corrupt());
    }
    Ok(Some(*facts))
}

async fn turn_at(
    store: &MongoSandboxStore,
    source: &capture::CapturedLog,
    session: &mut ClientSession,
    turn: u64,
) -> Result<AgentLogTurnFacts, SandboxStoreError> {
    let Some(projection::EvidenceRecord::Turn { facts }) = projection::evidence(
        store,
        source.active_head(),
        "turn",
        &turn.to_string(),
        session,
    )
    .await
    .map_err(segment_error)?
    else {
        return Err(corrupt());
    };
    if facts.turn_sequence != turn {
        return Err(corrupt());
    }
    Ok(*facts)
}

async fn points(
    store: &MongoSandboxStore,
    source: &capture::CapturedLog,
    session: &mut ClientSession,
    sequences: &[u64],
) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
    capture::points(
        store,
        source,
        sequences,
        session,
        capture::CaptureMode::Snapshot,
    )
    .await
    .map_err(segment_error)
}
fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("transcript evidence does not match its certified source".into())
}
