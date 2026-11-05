use layer2_core::sandbox::agent_io::{AdmittedAgentCommand, CommandEvidence, CommandLogReferences};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, AgentLogEntry, AgentLogProjectionState};
use mongodb::{bson::doc, options::ReadConcern, ClientSession};

use super::{
    corrupt,
    projection::{self, EvidenceRecord},
    read_format, read_head, visit_range, Head,
};
use crate::store::{
    agent_log_integrity::TransactionLogError, agents::read_agent, sandbox_mongo::MongoSandboxStore,
};

type Result<T> = std::result::Result<T, TransactionLogError>;

pub(in crate::store) async fn title_transcript(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Option<Option<layer2_core::sandbox::AgentTitleTranscript>>> {
    let Some((head, state)) = semantic(store, agent, session, snapshot).await? else {
        return Ok(None);
    };
    let Some(sequence) = state.title_turn else {
        return Ok(Some(None));
    };
    let Some(EvidenceRecord::Turn { facts }) =
        projection::evidence(store, &head, "turn", &sequence.to_string(), session).await?
    else {
        return Err(corrupt().into());
    };
    if !facts.source_completed || !facts.title_eligible {
        return Ok(Some(None));
    }
    let (Some(owner), Some(assistant), Some(end)) = (
        facts.owner_sequence,
        facts.first_assistant_sequence,
        facts.end_sequence,
    ) else {
        return Ok(Some(None));
    };
    if owner <= facts.turn_sequence || assistant <= owner || end <= assistant {
        return Ok(Some(None));
    }
    let entries = points(
        store,
        &head,
        &[facts.turn_sequence, owner, assistant, end],
        session,
        snapshot,
    )
    .await?;
    Ok(Some(
        layer2_core::sandbox::AgentTitleTranscript::from_complete_log(&entries),
    ))
}

pub(in crate::store) async fn semantic(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Option<(Head, AgentLogProjectionState)>> {
    let find = store.agents.find_one(doc! {"_id":agent.id.as_str()});
    let find = if snapshot {
        find.read_concern(ReadConcern::snapshot())
    } else {
        find
    };
    let row = find
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(corrupt)?;
    if read_format(&row)?.is_none() {
        return Ok(None);
    }
    if read_agent(&row)? != *agent {
        return Err(SandboxStoreError::Conflict.into());
    }
    let head = read_head(store, &row, session, snapshot).await?;
    let state = projection::read(store, agent, &head, session).await?;
    Ok(Some((head, state)))
}

/// Exact receipt references share a single active-tail scan. Original source
/// events are returned in order, bounded independently of lifetime history.
pub(super) async fn points(
    store: &MongoSandboxStore,
    head: &Head,
    sequences: &[u64],
    session: &mut ClientSession,
    snapshot: bool,
) -> Result<Vec<AgentLogEntry>> {
    let mut entries = Vec::with_capacity(sequences.len());
    visit_points(store, head, sequences, session, snapshot, &mut |entry| {
        entries.push(entry);
        Ok(())
    })
    .await?;
    Ok(entries)
}

pub(super) async fn visit_points(
    store: &MongoSandboxStore,
    head: &Head,
    sequences: &[u64],
    session: &mut ClientSession,
    snapshot: bool,
    visit: &mut (dyn FnMut(AgentLogEntry) -> std::result::Result<(), SandboxStoreError> + Send),
) -> Result<()> {
    if sequences.len() > 200
        || sequences
            .iter()
            .any(|n| *n == 0 || *n > head.last_seq as u64)
    {
        return Err(corrupt().into());
    }
    let wanted: std::collections::BTreeSet<_> = sequences.iter().copied().collect();
    let mut count = 0;
    for sequence in wanted.iter().filter(|n| **n < head.active_start_seq as u64) {
        visit_range(
            store,
            head,
            super::LogRange {
                after: sequence - 1,
                through: *sequence,
                limit: 1,
            },
            session,
            snapshot,
            |entry| {
                count += 1;
                visit(entry)
            },
        )
        .await?;
    }
    if wanted.iter().any(|n| *n >= head.active_start_seq as u64) {
        visit_range(
            store,
            head,
            super::LogRange {
                after: head.active_start_seq as u64 - 1,
                through: head.last_seq as u64,
                limit: head.active_count as u16,
            },
            session,
            snapshot,
            |entry| {
                if wanted.contains(&entry.seq) {
                    count += 1;
                    visit(entry)?;
                }
                Ok(())
            },
        )
        .await?;
    }
    if count != wanted.len() {
        return Err(corrupt().into());
    }
    Ok(())
}

pub(in crate::store) async fn command_evidence(
    store: &MongoSandboxStore,
    agent: &Agent,
    command: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<Option<CommandEvidence>> {
    let Some((head, state)) = semantic(store, agent, session, false).await? else {
        return Ok(None);
    };
    let dispatch = command.work.dispatch.as_ref().ok_or_else(corrupt)?;
    let Some(EvidenceRecord::Turn { facts }) = projection::evidence(
        store,
        &head,
        "turn",
        &dispatch.turn_sequence.to_string(),
        session,
    )
    .await?
    else {
        return Err(corrupt().into());
    };
    let Some(EvidenceRecord::Operation {
        operation_sequence,
        sequence,
    }) = projection::evidence(
        store,
        &head,
        "operation",
        &dispatch.operation_sequence.to_string(),
        session,
    )
    .await?
    else {
        return Err(corrupt().into());
    };
    if operation_sequence != dispatch.operation_sequence
        || facts.turn_sequence != dispatch.turn_sequence
    {
        return Err(corrupt().into());
    }
    let pending_row = store
        .segment_logs
        .evidence
        .find_one(doc! {
            "agent_id":&head.agent_id,"incarnation":&head.incarnation,"evidence_generation":&head.evidence_generation,"kind":"approval",
            "record.epoch":i64::try_from(state.approval_epoch).map_err(|_|corrupt())?,
            "record.turn_sequence":i64::try_from(dispatch.turn_sequence).map_err(|_|corrupt())?,
            "record.request_sequence":{"$gt":0_i64},
        })
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?;
    let pending = pending_row
        .map(|row| {
            let key = row.get_str("entity_key").map_err(|_| corrupt())?.to_owned();
            match projection::read_evidence(&head, "approval", &key, row)? {
                EvidenceRecord::Approval {
                    approval_id,
                    request_sequence: Some(sequence),
                    turn_sequence: Some(turn),
                    epoch,
                } if approval_id == key
                    && turn == dispatch.turn_sequence
                    && epoch == state.approval_epoch
                    && sequence > turn =>
                {
                    Ok((sequence, key))
                }
                _ => Err(corrupt()),
            }
        })
        .transpose()?;
    let mut wanted = vec![
        dispatch.turn_sequence,
        dispatch.operation_sequence,
        sequence,
    ];
    wanted.extend(facts.end_sequence);
    wanted.extend(pending.as_ref().map(|(sequence, _)| *sequence));
    let entries = points(store, &head, &wanted, session, false).await?;
    let get = |seq| {
        entries
            .iter()
            .find(|e| e.seq == seq)
            .cloned()
            .ok_or_else(corrupt)
    };
    if let Some((sequence, key)) = &pending {
        if !matches!(&get(*sequence)?.kind,
            layer2_core::sandbox::AgentLogKind::ApprovalRequested { approval_id, .. }
                if approval_id == key)
        {
            return Err(corrupt().into());
        }
    }
    let proof = CommandLogReferences {
        through_seq: head.last_seq as u64,
        turn: get(dispatch.turn_sequence)?,
        fence: get(dispatch.operation_sequence)?,
        latest_delivery: get(sequence)?,
        terminal: facts.end_sequence.map(get).transpose()?,
        has_pending_approvals: pending.is_some(),
    };
    CommandEvidence::from_projection(command, proof)
        .map(Some)
        .map_err(|_| TransactionLogError::Domain(corrupt()))
}
