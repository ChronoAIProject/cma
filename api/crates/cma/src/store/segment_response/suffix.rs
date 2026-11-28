//! Extend a verified legacy turn only after proving its original boundary.
use super::*;

pub(super) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    previous: &AgentResponseLogWindow,
) -> Result<Option<AgentResponseLogWindow>, SandboxStoreError> {
    if !store.bounded_response_reads {
        return Ok(None);
    }
    let Some(turn) = previous.turn_sequence else {
        return Ok(None);
    };
    let Some(tail) = previous.entries.last() else {
        return Ok(None);
    };
    if turn == 0
        || tail.seq < turn
        || tail.seq > previous.through
        || tail.agent_id != agent.id
        || previous.through >= agent.conversation.last_seq
    {
        return Err(SandboxStoreError::SequenceConflict);
    }
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let (head, captured) = read_response_head(store, &agent.id, &mut session).await?;
    if captured.revision == agent.revision && captured != *agent {
        return Err(SandboxStoreError::SequenceConflict);
    }
    if head.last_seq != agent.conversation.last_seq {
        return Err(advance::legacy(
            store,
            agent,
            &captured,
            &head,
            Some(turn),
            Some(tail),
            &mut session,
        )
        .await?);
    }
    if !head.trusted {
        return Ok(None);
    }
    let closed = tail.seq > turn && matches!(tail.kind, AgentLogKind::TurnStarted { .. });
    let through = if closed {
        tail.seq
    } else {
        let bound = head.last_seq.min(turn.saturating_add(MAX_ENTRIES - 1));
        if tail.seq > bound {
            return Ok(None);
        }
        let next = boundary::next_turn(store, &agent.id, tail.seq, bound, &mut session).await?;
        if next.is_none() && bound != head.last_seq {
            return Ok(None);
        }
        next.unwrap_or(bound)
    };
    let Some(suffix) = read_bounded(
        store,
        &agent.id,
        tail.seq - 1,
        through,
        &head,
        &mut session,
        MAX_BYTES,
    )
    .await?
    else {
        return Ok(None);
    };
    if suffix.first() != Some(tail) {
        return Err(SandboxStoreError::Corrupt(
            "retained output boundary changed".into(),
        ));
    }
    let mut entries = previous.entries.clone();
    entries.extend(suffix.into_iter().skip(1));
    let bytes = entries.iter().try_fold(0usize, |total, entry| {
        crate::store::agent_log::log_document(entry).and_then(|row| {
            row.to_vec()
                .map(|bytes| total.saturating_add(bytes.len()))
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        })
    })?;
    if entries.len() > MAX_ENTRIES as usize || bytes > MAX_BYTES {
        return Ok(None);
    }
    Ok(Some(AgentResponseLogWindow {
        through: head.last_seq,
        turn_sequence: Some(turn),
        entries,
    }))
}
