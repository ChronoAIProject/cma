//! Read-only settlement inside the caller's existing transaction. No transaction
//! lifecycle, writes, reusable capture token or whole-history fallback.
use super::{
    collections::collection,
    corrupt, ordinary,
    projection::{self, EvidenceRecord},
    Head, EVIDENCE,
};
use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::MongoSandboxStore};
use layer2_core::sandbox::{
    agent_io::{AdmittedAgentCommand, AgentCommand, CertifiedCommandSource, CommandEvidence},
    Agent, AgentLogEntry, AgentLogKind,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};

type Result<T> = std::result::Result<T, TransactionLogError>;

pub(in crate::store) async fn read_transaction(
    store: &MongoSandboxStore,
    agent: &Agent,
    agent_row: &Document,
    command: &AdmittedAgentCommand,
    session: &mut ClientSession,
) -> Result<Option<CommandEvidence>> {
    let Some((head, state)) = ordinary::source(store, agent, agent_row, session).await? else {
        return Ok(None);
    };
    let dispatch = command.work.dispatch.as_ref().ok_or_else(corrupt)?;
    if dispatch.claim.agent_id != agent.id {
        return Err(corrupt().into());
    }
    let turn = dispatch.turn_sequence;
    let Some(EvidenceRecord::Turn { facts }) =
        projection::evidence(store, &head, "turn", &turn.to_string(), session).await?
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
        &head,
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
    let start = ordinary::point(store, &head, turn, session).await?;
    let fence = ordinary::point(store, &head, operation_sequence, session).await?;
    let delivery = if sequence == operation_sequence {
        fence.clone()
    } else {
        ordinary::point(store, &head, sequence, session).await?
    };
    let terminal = optional_point(store, &head, facts.end_sequence, session).await?;
    let is_message = matches!(
        command.receipt.envelope.command,
        AgentCommand::Message { .. }
    );
    let prewrite = optional_point(
        store,
        &head,
        is_message
            .then_some(facts.prewrite_failure_sequence)
            .flatten(),
        session,
    )
    .await?;
    let last_approval =
        optional_point(store, &head, facts.approval_change_sequence, session).await?;
    let pending = if is_message && facts.end_sequence.is_none() {
        // The epoch's range is indexed. One member proves nonemptiness without
        // collecting the approval set or scanning lifetime resolved approvals.
        pending_approval(store, &head, turn, state.approval_epoch, session).await?
    } else {
        None
    };
    CommandEvidence::from_certified_source(
        command,
        CertifiedCommandSource {
            through: state.through_seq,
            facts: &facts,
            start: &start,
            fence: &fence,
            delivery: &delivery,
            terminal: terminal.as_ref(),
            prewrite: prewrite.as_ref(),
            last_approval: last_approval.as_ref(),
            pending_approval: pending.as_ref(),
        },
    )
    .map(Some)
    .map_err(|_| corrupt().into())
}

async fn optional_point(
    store: &MongoSandboxStore,
    head: &Head,
    sequence: Option<u64>,
    session: &mut ClientSession,
) -> Result<Option<AgentLogEntry>> {
    match sequence {
        Some(sequence) => ordinary::point(store, head, sequence, session)
            .await
            .map(Some),
        None => Ok(None),
    }
}

async fn pending_approval(
    store: &MongoSandboxStore,
    head: &Head,
    turn: u64,
    epoch: u64,
    session: &mut ClientSession,
) -> Result<Option<AgentLogEntry>> {
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
    let entry = ordinary::point(store, head, sequence, session).await?;
    if !matches!(&entry.kind, AgentLogKind::ApprovalRequested { approval_id, .. } if approval_id == &key)
    {
        return Err(corrupt().into());
    }
    Ok(Some(entry))
}
