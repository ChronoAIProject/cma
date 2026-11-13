use super::*;

pub(in crate::responses) async fn project_metadata(
    source: &mut AgentOutputObservation<'_>,
    origin: &crate::agent_page::AgentPageOrigin,
    reading: AgentResponseReading,
) -> Result<V2ResponseObject, AgentReadError> {
    let metadata = source.metadata().clone();
    let mut history = vec![];
    if let Some(turn) = &metadata.turn {
        let mut sequences = vec![turn.facts.turn_sequence];
        sequences.extend(
            [
                turn.terminal_sequence,
                turn.prewrite_sequence,
                turn.usage_sequence,
                turn.facts.metadata_sequence,
            ]
            .into_iter()
            .flatten(),
        );
        sequences.sort_unstable();
        sequences.dedup();
        history = source.points(&sequences).await?;
    }
    let snapshot = AgentEventStreamSnapshot {
        id: metadata.agent_id,
        created_at: metadata.created_at,
        initial_model: metadata.initial_model,
        model: None,
        runtime_generation: None,
        active_turn: None,
        head: metadata.through,
        history,
    };
    // Every row is original and point-proven. Only metadata is projected here;
    // output is independently indexed, not fabricated as a shortened turn log.
    let mut response = project(origin, reading, &snapshot).ok_or_else(corrupt)?;
    if let Some(turn) = metadata.turn {
        if turn.facts.model.is_none() && !turn.legacy_model_known {
            response.model = "unknown".into();
        }
        if turn.facts.lost_frames > 0 && turn.terminal_sequence.is_some() {
            response.status = V2ResponseStatus::Incomplete;
            response.incomplete_details =
                Some(json!({"reason":"conversation_output_gap","frames":turn.facts.lost_frames}));
        }
    }
    Ok(response)
}
