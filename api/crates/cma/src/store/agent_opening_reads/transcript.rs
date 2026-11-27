use super::*;
use layer2_core::sandbox::{
    AgentLogKind, AgentLogProjectionState, AgentTranscript, AgentTranscriptFold,
    AgentTranscriptMessage, AgentTranscriptPage, AgentTranscriptPageRequest, AgentTranscriptRole,
    AgentTranscriptWindow,
};

// Preserve the indexed reader for qualified histories. For a legacy history
// without that optional index or proof, verify a bounded source snapshot.
pub(in crate::store) async fn export_page(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    request: &AgentTranscriptPageRequest,
) -> Result<Option<AgentTranscriptPage>, SandboxStoreError> {
    use crate::store::agent_log_integrity::{
        read::read_proven_agent, scan_collection, ScanOptions, TransactionLogError,
    };
    use crate::store::agent_log_telemetry::ScanCaller;

    if let Some(page) = page(store, agent, session, request).await? {
        return Ok(Some(page));
    }
    let (observed, _) = read_proven_agent(store, &agent.id, session)
        .await?
        .ok_or(SandboxStoreError::NotFound)?;
    if observed != *agent {
        return Err(SandboxStoreError::SequenceConflict);
    }
    let mut fold = AgentTranscriptFold::default();
    scan_collection(
        ScanCaller::PublicWindowPrefix,
        &store.agent_log,
        &agent.id,
        agent.conversation.last_seq,
        session,
        ScanOptions::new(true, store.scan_cooperation).with_limits(1_024, 8 * 1024 * 1024),
        |entry| fold.push(agent, entry).map_err(SandboxStoreError::Corrupt),
    )
    .await
    .map_err(|error| match error {
        TransactionLogError::Domain(error) => error,
        TransactionLogError::Driver(error) => unavailable(error),
    })?;
    fold.finish(agent)
        .map_err(SandboxStoreError::Corrupt)?
        .into_page(request)
        .map(Some)
        .map_err(|_| SandboxStoreError::InvalidPage)
}

pub(in crate::store) async fn page(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    request: &AgentTranscriptPageRequest,
) -> Result<Option<AgentTranscriptPage>, SandboxStoreError> {
    if !store.bounded_transcript_reads
        && std::env::var("CHRONO_AGENT_TRANSCRIPT_BOUNDED_READS_ENABLED").as_deref() != Ok("true")
    {
        return Ok(None);
    }
    request
        .validate()
        .map_err(|_| SandboxStoreError::InvalidPage)?;
    if request
        .through
        .is_some_and(|seq| seq != agent.conversation.last_seq)
    {
        return Err(SandboxStoreError::SequenceConflict);
    }
    if !eligible(store, agent, session).await? {
        return Ok(None);
    }
    let head = agent.conversation.last_seq;
    let start = match request.turn {
        Some(turn) => selected(
            store,
            agent,
            session,
            Selection {
                types: &[],
                after: turn - 1,
                through: turn,
                limit: 1,
                descending: false,
            },
        )
        .await?
        .pop()
        .ok_or_else(corrupt)?,
        None => {
            let Some(start) = next_turn(store, agent, session, 0).await? else {
                decision("transcript_empty");
                return Ok(Some(AgentTranscriptPage {
                    transcript: AgentTranscript {
                        agent_id: agent.id.clone(),
                        through_seq: head,
                        turns: vec![],
                    },
                    next: None,
                }));
            };
            start
        }
    };
    let turn = start.seq;
    if !matches!(start.kind, AgentLogKind::TurnStarted { turn_sequence, .. } if turn_sequence == turn)
    {
        return Err(corrupt());
    }
    let next_turn = next_turn(store, agent, session, turn).await?;
    let end = next_turn.as_ref().map_or(head, |row| row.seq - 1);
    let after = request.after.unwrap_or(turn - 1);
    if after < turn - 1 || after >= end {
        return Err(SandboxStoreError::InvalidPage);
    }
    let terminal = one(
        store,
        agent,
        session,
        &["turn_completed", "turn_failed", "turn_interrupted"],
        turn,
        end,
    )
    .await?;
    if terminal.is_none() && next_turn.is_some() {
        return Err(corrupt());
    }
    let active_end = terminal.as_ref().map_or(end, |row| row.seq - 1);
    let owner = if let Some(owner) = AgentLogProjectionState::queued_owner(&start) {
        selected(
            store,
            agent,
            session,
            Selection {
                types: &[],
                after: owner.checked_sub(1).ok_or_else(corrupt)?,
                through: owner,
                limit: 1,
                descending: false,
            },
        )
        .await?
        .pop()
        .ok_or_else(corrupt)?
    } else {
        selected(
            store,
            agent,
            session,
            Selection {
                types: &["owner_message"],
                after: turn,
                through: active_end,
                limit: 1,
                descending: false,
            },
        )
        .await?
        .pop()
        .ok_or_else(corrupt)?
    };
    let metadata = one(
        store,
        agent,
        session,
        &["turn_metadata_set"],
        turn,
        active_end,
    )
    .await?;
    let gaps = selected(
        store,
        agent,
        session,
        Selection {
            types: &["conversation_gap"],
            after: turn,
            through: active_end,
            limit: 33,
            descending: false,
        },
    )
    .await?;
    if gaps.len() > 32 {
        decision("gap_budget");
        return Ok(None);
    }
    let window = AgentTranscriptWindow {
        start,
        owner,
        metadata,
        terminal,
        gaps,
    };
    let mut value = window.header(agent).map_err(SandboxStoreError::Corrupt)?;
    if after != turn - 1 {
        value.messages.clear();
    }
    let through = end.min(after.saturating_add(u64::from(request.limit)));
    let rows = selected(
        store,
        agent,
        session,
        Selection {
            types: &[],
            after,
            through,
            limit: i64::from(request.limit),
            descending: false,
        },
    )
    .await?;
    if rows.len() as u64 != through - after
        || rows
            .iter()
            .enumerate()
            .any(|(index, row)| row.seq != after + index as u64 + 1)
    {
        return Err(SandboxStoreError::SequenceConflict);
    }
    for row in rows {
        let (role, content) = match row.kind {
            AgentLogKind::OwnerMessage { content } if row.seq <= active_end => {
                (AgentTranscriptRole::Owner, content)
            }
            AgentLogKind::AgentMessage { content } if row.seq <= active_end => {
                (AgentTranscriptRole::Agent, content)
            }
            _ => continue,
        };
        if role == AgentTranscriptRole::Agent {
            value.agent_messages.push(content.clone());
        }
        value.messages.push(AgentTranscriptMessage {
            sequence: row.seq,
            role,
            content,
            appended_at: row.appended_at,
        });
    }
    let next = if through < end {
        Some((turn, through))
    } else {
        next_turn.map(|row| (row.seq, row.seq - 1))
    }
    .map(|(turn, after)| AgentTranscriptPageRequest {
        turn: Some(turn),
        after: Some(after),
        through: Some(head),
        limit: request.limit,
    });
    let result = AgentTranscriptPage {
        transcript: AgentTranscript {
            agent_id: agent.id.clone(),
            through_seq: head,
            turns: vec![value],
        },
        next,
    };
    if serde_json::to_vec(&result).map_err(|_| corrupt())?.len() > MAX_BYTES {
        return Err(SandboxStoreError::AgentLogCapacityExceeded);
    }
    decision("transcript_window");
    Ok(Some(result))
}

async fn next_turn(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    after: u64,
) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
    Ok(selected(
        store,
        agent,
        session,
        Selection {
            types: &["turn_started"],
            after,
            through: agent.conversation.last_seq,
            limit: 1,
            descending: false,
        },
    )
    .await?
    .pop())
}

async fn one(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    types: &[&str],
    after: u64,
    through: u64,
) -> Result<Option<AgentLogEntry>, SandboxStoreError> {
    let mut rows = selected(
        store,
        agent,
        session,
        Selection {
            types,
            after,
            through,
            limit: 2,
            descending: false,
        },
    )
    .await?;
    if rows.len() > 1 {
        return Err(corrupt());
    }
    Ok(rows.pop())
}
