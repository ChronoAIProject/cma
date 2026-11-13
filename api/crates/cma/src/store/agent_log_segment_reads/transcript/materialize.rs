use super::*;

pub(super) async fn header(
    store: &MongoSandboxStore,
    source: &capture::CapturedLog,
    session: &mut ClientSession,
    facts: &AgentLogTurnFacts,
) -> Result<AgentTranscriptTurn, SandboxStoreError> {
    facts.validate(source.through()).map_err(|_| corrupt())?;
    let owner = facts.owner_sequence.ok_or_else(corrupt)?;
    let mut sequences = vec![facts.turn_sequence, owner];
    sequences.extend(facts.metadata_sequence);
    sequences.extend(facts.end_sequence);
    sequences.sort_unstable();
    sequences.dedup();
    let rows = points(store, source, session, &sequences).await?;
    let start = rows
        .iter()
        .find(|row| row.seq == facts.turn_sequence)
        .ok_or_else(corrupt)?;
    if !matches!(&start.kind, AgentLogKind::TurnStarted { turn_sequence, origin_key, model }
        if *turn_sequence == facts.turn_sequence && origin_key == &facts.origin_key && model == &facts.model)
        || start.appended_at != facts.started_at
    {
        return Err(corrupt());
    }
    let owner = rows
        .iter()
        .find(|row| row.seq == owner)
        .ok_or_else(corrupt)?;
    let AgentLogKind::OwnerMessage { content } = &owner.kind else {
        return Err(corrupt());
    };
    let mut metadata = Default::default();
    if let Some(seq) = facts.metadata_sequence {
        let row = rows.iter().find(|row| row.seq == seq).ok_or_else(corrupt)?;
        let AgentLogKind::TurnMetadataSet {
            turn_sequence,
            metadata: value,
        } = &row.kind
        else {
            return Err(corrupt());
        };
        if *turn_sequence != facts.turn_sequence {
            return Err(corrupt());
        }
        metadata = value.clone();
    }
    if let Some(seq) = facts.end_sequence {
        let row = rows.iter().find(|row| row.seq == seq).ok_or_else(corrupt)?;
        use layer2_core::sandbox::AgentTurnOutcome as Outcome;
        let valid = match (&row.kind, &facts.outcome) {
            (AgentLogKind::TurnCompleted { turn_sequence }, Outcome::Completed { at }) => {
                *turn_sequence == facts.turn_sequence
                    && at == &row.appended_at
                    && facts.source_completed
            }
            (AgentLogKind::TurnCompleted { turn_sequence }, Outcome::Failed { at, .. }) => {
                *turn_sequence == facts.turn_sequence
                    && at == &row.appended_at
                    && facts.source_completed
                    && facts.lost_frames > 0
            }
            (
                AgentLogKind::TurnFailed {
                    turn_sequence,
                    failure,
                },
                Outcome::Failed {
                    at,
                    failure: actual,
                },
            ) => {
                *turn_sequence == facts.turn_sequence && at == &row.appended_at && failure == actual
            }
            (AgentLogKind::TurnInterrupted { turn_sequence }, Outcome::Interrupted { at }) => {
                *turn_sequence == facts.turn_sequence && at == &row.appended_at
            }
            _ => false,
        };
        if !valid {
            return Err(corrupt());
        }
    }
    Ok(AgentTranscriptTurn {
        turn_sequence: facts.turn_sequence,
        started_at: facts.started_at.clone(),
        origin_key: facts.origin_key.clone(),
        owner_message: content.clone(),
        agent_messages: vec![],
        messages: if owner.seq < facts.turn_sequence {
            vec![AgentTranscriptMessage {
                sequence: owner.seq,
                role: AgentTranscriptRole::Owner,
                content: content.clone(),
                appended_at: owner.appended_at.clone(),
            }]
        } else {
            vec![]
        },
        metadata,
        outcome: facts.outcome.clone(),
    })
}

pub(super) fn append(
    facts: &AgentLogTurnFacts,
    turn: &mut AgentTranscriptTurn,
    entries: Vec<AgentLogEntry>,
    budget: usize,
) -> Result<usize, SandboxStoreError> {
    let mut bytes = serde_json::to_vec(turn).map_err(|_| corrupt())?.len();
    if bytes > budget {
        return Err(SandboxStoreError::AgentLogCapacityExceeded);
    }
    let mut admitted = 0;
    for entry in entries {
        if entry.seq < facts.turn_sequence || facts.end_sequence.is_some_and(|end| entry.seq > end)
        {
            return Err(corrupt());
        }
        let (role, content) = match entry.kind {
            AgentLogKind::OwnerMessage { content } => (AgentTranscriptRole::Owner, content),
            AgentLogKind::AgentMessage { content } => (AgentTranscriptRole::Agent, content),
            AgentLogKind::TurnStarted { turn_sequence, .. }
                if turn_sequence != facts.turn_sequence =>
            {
                return Err(corrupt())
            }
            _ => {
                admitted += 1;
                continue;
            }
        };
        let message = AgentTranscriptMessage {
            sequence: entry.seq,
            role,
            content,
            appended_at: entry.appended_at,
        };
        let mut added = serde_json::to_vec(&message).map_err(|_| corrupt())?.len()
            + usize::from(!turn.messages.is_empty());
        if role == AgentTranscriptRole::Agent {
            added += serde_json::to_vec(&message.content)
                .map_err(|_| corrupt())?
                .len()
                + usize::from(!turn.agent_messages.is_empty());
        }
        if bytes + added > budget {
            break;
        }
        bytes += added;
        if role == AgentTranscriptRole::Agent {
            turn.agent_messages.push(message.content.clone());
        }
        turn.messages.push(message);
        admitted += 1;
    }
    Ok(admitted)
}
