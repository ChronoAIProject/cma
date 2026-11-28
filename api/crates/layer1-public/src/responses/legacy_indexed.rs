//! Legacy wire projection from the same certified turn and original points.
use super::*;
use crate::responses::v2::projection::indexed::{corrupt, seek_message};
use layer2_core::sandbox::store::indexed::OUTPUT_PAGE_BYTES;
use layer2_core::sandbox::{AgentLogEntry, AgentOutputObservation, AgentReadError};

pub(in crate::responses) async fn metadata(
    source: &mut AgentOutputObservation<'_>,
    origin: &crate::agent_page::AgentPageOrigin,
    agent: &Agent,
    turn: u64,
) -> Result<ResponseObject, AgentReadError> {
    let metadata = source.metadata().clone();
    let facts = metadata.turn.as_ref().ok_or_else(corrupt)?;
    if facts.facts.turn_sequence != turn || metadata.agent_id != agent.id {
        return Err(corrupt());
    }
    let mut sequences = vec![turn];
    sequences.extend(
        [
            facts.terminal_sequence,
            facts.usage_sequence,
            facts.facts.metadata_sequence,
        ]
        .into_iter()
        .flatten(),
    );
    sequences.sort_unstable();
    sequences.dedup();
    let rows = source
        .points(&sequences)
        .await?
        .into_iter()
        .filter_map(|row| {
            Some(AgentLogEntry {
                agent_id: agent.id.clone(),
                seq: row.seq,
                appended_at: row.appended_at,
                kind: row.kind?,
            })
        })
        .collect::<Vec<_>>();
    let mut captured = agent.clone();
    captured.profile.content.model = metadata.initial_model;
    let mut response = project(origin, &captured, &rows, turn).ok_or_else(corrupt)?;
    response.previous_response_id = facts
        .facts
        .previous_turn
        .map(|previous| response_id(&agent.id, previous));
    if facts.facts.model.is_none() && !facts.legacy_model_known {
        response.model = "unknown".into();
    }
    if facts.facts.lost_frames > 0 && facts.terminal_sequence.is_some() {
        response.status = ResponseStatus::Incomplete;
        response.incomplete_details =
            Some(json!({"reason":"conversation_output_gap","frames":facts.facts.lost_frames}));
    }
    Ok(response)
}

pub(in crate::responses) async fn messages(
    source: &mut AgentOutputObservation<'_>,
    agent: &Agent,
    turn: u64,
    from: u64,
    limit: u16,
) -> Result<(Vec<OutputMessage>, u64, bool), AgentReadError> {
    let index = source
        .metadata()
        .index
        .as_ref()
        .ok_or_else(corrupt)?
        .clone();
    if from > index.totals.messages || limit == 0 || limit > 8 {
        return Err(AgentReadError::InvalidPage);
    }
    let through = index.totals.messages.min(from.saturating_add(1));
    let mut messages = vec![];
    let mut bytes = 2048;
    for ordinal in from..through {
        let change = seek_message(source, ordinal).await?;
        let rows = source.points(&[change.source_sequence]).await?;
        let Some(AgentLogKind::AgentMessage { content }) =
            rows.first().and_then(|r| r.kind.as_ref())
        else {
            return Err(corrupt());
        };
        let message = output_message(
            agent,
            turn,
            usize::try_from(ordinal).map_err(|_| corrupt())?,
            content,
        );
        let size = serde_json::to_vec(&message).map_err(|_| corrupt())?.len();
        if size + 2048 > OUTPUT_PAGE_BYTES {
            return Err(AgentReadError::Unavailable(
                "one output item exceeds the page budget".into(),
            ));
        }
        if bytes + size > OUTPUT_PAGE_BYTES {
            break;
        }
        bytes += size;
        messages.push(message);
    }
    let next = from + messages.len() as u64;
    Ok((messages, next, next < index.totals.messages))
}

pub(in crate::responses) async fn complete(
    source: &mut AgentOutputObservation<'_>,
    origin: &crate::agent_page::AgentPageOrigin,
    agent: &Agent,
    turn: u64,
) -> Result<Option<ResponseObject>, AgentReadError> {
    let mut response = metadata(source, origin, agent, turn).await?;
    let mut from = 0;
    let mut bytes = serde_json::to_vec(&response).map_err(|_| corrupt())?.len();
    loop {
        let (messages, next, more) = messages(source, agent, turn, from, 8).await?;
        for message in messages {
            bytes = bytes
                .checked_add(serde_json::to_vec(&message).map_err(|_| corrupt())?.len() + 1)
                .ok_or_else(corrupt)?;
            if bytes > OUTPUT_PAGE_BYTES {
                return Ok(None);
            }
            response.output.push(message);
        }
        if !more {
            break;
        }
        if next <= from {
            return Err(corrupt());
        }
        from = next;
    }
    Ok(Some(response))
}

pub(in crate::responses) async fn retrieve_value(
    source: &mut AgentOutputObservation<'_>,
    origin: &crate::agent_page::AgentPageOrigin,
    agent: &Agent,
    turn: u64,
    paged: bool,
    from: u64,
) -> Result<Option<serde_json::Value>, AgentReadError> {
    if paged {
        let (messages, next, more) = messages(source, agent, turn, from, 8).await?;
        Ok(Some(
            json!({"object":"cma.response.output_page","response_id":response_id(&agent.id,turn),"data":messages,"has_more":more,"next_after":more.then_some(next),"through":source.metadata().through}),
        ))
    } else {
        complete(source, origin, agent, turn)
            .await?
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| corrupt())
    }
}
