use super::*;
use layer2_core::sandbox::AgentLogKind;

async fn tool_projection(
    source: &mut AgentOutputObservation<'_>,
    tool: &AgentToolOutputState,
) -> Result<OutputProjection, AgentReadError> {
    let mut sequences = vec![tool.start_sequence];
    sequences.extend(
        [tool.output_sequence, tool.terminal_sequence]
            .into_iter()
            .flatten(),
    );
    sequences.sort_unstable();
    sequences.dedup();
    let rows = source.points(&sequences).await?;
    if !rows.first().is_some_and(|row| {
        matches!(&row.kind,
        Some(AgentLogKind::ToolCallStarted {call_id,..}) if call_id==&tool.call_id)
    }) {
        return Err(corrupt());
    }
    let mut projection = OutputProjection::at(
        usize::try_from(tool.item_ordinal).map_err(|_| corrupt())?,
        0,
    );
    for row in rows {
        let kind = row.kind.ok_or_else(corrupt)?;
        let call = match &kind {
            AgentLogKind::ToolCallStarted { call_id, .. }
            | AgentLogKind::ToolCallUpdated { call_id, .. }
            | AgentLogKind::ToolOutputReported { call_id, .. } => call_id,
            _ => return Err(corrupt()),
        };
        if call != &tool.call_id {
            return Err(corrupt());
        }
        if tool.terminal_sequence == Some(row.seq)
            && !matches!(&kind,
            AgentLogKind::ToolCallStarted {status,..}|AgentLogKind::ToolCallUpdated {status,..} if matches!(status.as_str(),"completed"|"failed"))
        {
            return Err(corrupt());
        }
        projection.observe(&kind, &source.metadata().agent_id, tool.turn_sequence);
    }
    Ok(projection)
}

async fn message_projection(
    source: &mut AgentOutputObservation<'_>,
    change: &AgentOutputChange,
) -> Result<OutputProjection, AgentReadError> {
    let rows = source.points(&[change.source_sequence]).await?;
    let kind = rows
        .first()
        .and_then(|r| r.kind.as_ref())
        .ok_or_else(corrupt)?;
    let AgentLogKind::AgentMessage { content } = kind else {
        return Err(corrupt());
    };
    if change.after.nonempty_messages - change.before.nonempty_messages
        != u64::from(!content.is_empty())
    {
        return Err(corrupt());
    }
    let mut projection = OutputProjection::at(
        usize::try_from(change.item_ordinal).map_err(|_| corrupt())?,
        usize::try_from(change.message_ordinal.ok_or_else(corrupt)?).map_err(|_| corrupt())?,
    );
    projection.observe(kind, &source.metadata().agent_id, change.turn_sequence);
    Ok(projection)
}

pub(in crate::responses) async fn current_item(
    source: &mut AgentOutputObservation<'_>,
    change: &AgentOutputChange,
) -> Result<V2OutputItem, AgentReadError> {
    let mut projection = if let Some(original) = &change.tool {
        let tool = source.tool(&original.call_id).await?;
        if tool.start_sequence != original.start_sequence
            || tool.item_ordinal != change.item_ordinal
        {
            return Err(corrupt());
        }
        tool_projection(source, &tool).await?
    } else {
        message_projection(source, change).await?
    };
    if projection.items.len() != 1 {
        return Err(corrupt());
    }
    Ok(projection.items.remove(0))
}

pub(in crate::responses) async fn change_events(
    source: &mut AgentOutputObservation<'_>,
    change: &AgentOutputChange,
) -> Result<Vec<Value>, AgentReadError> {
    let mut projection = if let Some(tool) = &change.tool {
        let mut projection = tool_projection(source, tool).await?;
        if tool.start_sequence != change.source_sequence {
            if tool.terminal_sequence != Some(change.source_sequence) || projection.events.len() < 2
            {
                return Err(corrupt());
            }
            projection.events.drain(..2);
        }
        projection
    } else {
        message_projection(source, change).await?
    };
    let before = event_count(&change.before)?;
    let after = event_count(&change.after)?;
    if after.checked_sub(before) != Some(projection.events.len() as u64) {
        return Err(corrupt());
    }
    for (offset, event) in projection.events.iter_mut().enumerate() {
        event["sequence_number"] = (2 + before + offset as u64).into();
    }
    Ok(projection.events)
}
