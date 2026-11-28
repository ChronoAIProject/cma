//! Read-only settlement inside the caller's existing transaction. No transaction
//! lifecycle, writes, reusable capture token or whole-history fallback.
use super::{
    collections::collection,
    corrupt,
    projection::{self, EvidenceRecord},
    read_format, read_head, Head, EVIDENCE,
};
use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::MongoSandboxStore};
use layer2_core::sandbox::{
    agent_io::{AdmittedAgentCommand, AgentCommand, CertifiedCommandSource, CommandEvidence},
    Agent, AgentLogKind, AgentLogProjectionState,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

mod points;
#[cfg(test)]
mod points_tests;

type Result<T> = std::result::Result<T, TransactionLogError>;

pub(in crate::store) async fn read_transaction(
    store: &MongoSandboxStore,
    agent: &Agent,
    agent_row: &Document,
    command: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<Option<CommandEvidence>> {
    let Some(source) = read_source(store, agent, agent_row, session).await? else {
        return Ok(None);
    };
    read_from_source(store, &source, command, session)
        .await
        .map(Some)
}

pub(in crate::store) struct EvidenceSource {
    head: Head,
    state: AgentLogProjectionState,
}

/// The audit reuses this proof only inside its read-only snapshot transaction.
pub(in crate::store) async fn read_source(
    store: &MongoSandboxStore,
    agent: &Agent,
    agent_row: &Document,
    session: &mut ClientSession,
) -> Result<Option<EvidenceSource>> {
    if crate::store::agents::read_agent(agent_row)? != *agent {
        return Err(corrupt().into());
    }
    if read_format(agent_row)?.is_none() {
        return Ok(None);
    }
    let head = read_head(store, agent_row, session, false).await?;
    let state = projection::read(store, agent, &head, session).await?;
    Ok(Some(EvidenceSource { head, state }))
}

pub(in crate::store) async fn read_from_source(
    store: &MongoSandboxStore,
    source: &EvidenceSource,
    command: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<CommandEvidence> {
    let EvidenceSource { head, state } = source;
    let dispatch = command.work.dispatch.as_ref().ok_or_else(corrupt)?;
    if dispatch.claim.agent_id.as_str() != head.agent_id {
        return Err(corrupt().into());
    }
    let turn = dispatch.turn_sequence;
    let Some(EvidenceRecord::Turn { facts }) =
        projection::evidence(store, head, "turn", &turn.to_string(), session).await?
    else {
        return Err(corrupt().into());
    };
    if facts.turn_sequence != turn
        || state
            .active
            .as_ref()
            .into_iter()
            .chain(state.latest_completed.as_ref())
            .any(|current| current.turn_sequence == turn && current != facts.as_ref())
        || (facts.end_sequence.is_none() && state.active.as_ref() != Some(facts.as_ref()))
    {
        return Err(corrupt().into());
    }
    let Some(EvidenceRecord::Operation {
        operation_sequence,
        sequence,
    }) = projection::evidence(
        store,
        head,
        "operation",
        &dispatch.operation_sequence.to_string(),
        session,
    )
    .await?
    else {
        return Err(corrupt().into());
    };
    if operation_sequence != dispatch.operation_sequence {
        return Err(corrupt().into());
    }
    let is_message = matches!(
        command.receipt.envelope.command,
        AgentCommand::Message { .. }
    );
    let prewrite = is_message
        .then_some(facts.prewrite_failure_sequence)
        .flatten();
    let pending = if is_message && facts.end_sequence.is_none() {
        // The epoch's range is indexed. One member proves nonemptiness without
        // collecting the approval set or scanning lifetime resolved approvals.
        pending_approval(store, head, turn, state.approval_epoch, session).await?
    } else {
        None
    };
    let requested = [
        Some(turn),
        Some(operation_sequence),
        Some(sequence),
        facts.end_sequence,
        prewrite,
        facts.approval_change_sequence,
        pending.as_ref().map(|(_, seq)| *seq),
    ];
    let rows = points::read(store, head, requested, session).await?;
    let point = |sequence: u64| rows.get(&sequence).ok_or_else(corrupt);
    let optional = |sequence: Option<u64>| sequence.map(point).transpose();
    let pending_row = optional(pending.as_ref().map(|(_, seq)| *seq))?;
    if pending.as_ref().zip(pending_row).is_some_and(|((key, _), row)|
        !matches!(&row.kind, AgentLogKind::ApprovalRequested { approval_id, .. } if approval_id == key)) {
        return Err(corrupt().into());
    }
    CommandEvidence::from_certified_source(
        command,
        CertifiedCommandSource {
            through: state.through_seq,
            facts: &facts,
            start: point(turn)?,
            fence: point(operation_sequence)?,
            delivery: point(sequence)?,
            terminal: optional(facts.end_sequence)?,
            prewrite: optional(prewrite)?,
            last_approval: optional(facts.approval_change_sequence)?,
            pending_approval: pending_row,
        },
    )
    .map_err(|_| corrupt().into())
}

async fn pending_approval(
    store: &MongoSandboxStore,
    head: &Head,
    turn: u64,
    epoch: u64,
    session: &mut ClientSession,
) -> Result<Option<(String, u64)>> {
    if epoch != turn {
        return Err(corrupt().into());
    }
    let Some(row) = collection(store, EVIDENCE)
        .find_one(doc! {
            "agent_id": &head.agent_id, "incarnation": &head.incarnation,
            "evidence_generation": &head.evidence_generation, "kind": "approval",
            "record.epoch": epoch as i64, "record.request_sequence": {"$gt": 0_i64},
        })
        .sort(doc! {"record.request_sequence":1,"entity_key":1})
        .session(&mut *session)
        .await
        .map_err(TransactionLogError::Driver)?
    else {
        return Ok(None);
    };
    let key = row.get_str("entity_key").map_err(|_| corrupt())?.to_owned();
    let EvidenceRecord::Approval {
        approval_id,
        request_sequence: Some(sequence),
        turn_sequence,
        epoch: actual,
    } = projection::read_evidence(head, "approval", &key, row)?
    else {
        return Err(corrupt().into());
    };
    if actual != epoch || turn_sequence != Some(turn) || approval_id != key {
        return Err(corrupt().into());
    }
    Ok(Some((key, sequence)))
}
