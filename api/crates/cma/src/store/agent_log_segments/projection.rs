//! Bounded semantic state and indexed source references shared by all writers.
use layer2_core::sandbox::agent_output_index::{
    AgentOutputChange, AgentToolOutputState, AgentTurnOutputIndex,
};
use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogEvidenceChange, AgentLogProjectionState,
    AgentLogTurnFacts, AgentTranscriptRole,
};
use mongodb::{
    bson::{self, doc, Document},
    ClientSession,
};
use serde::{Deserialize, Serialize};

#[path = "projection_output.rs"]
pub(super) mod output;

use super::{collections::collection, corrupt, Head, EVIDENCE, PROJECTIONS};
use crate::store::{agent_log_integrity::TransactionLogError, sandbox_mongo::MongoSandboxStore};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct Projection {
    #[serde(rename = "_id")]
    pub agent_id: String,
    pub format_version: i64,
    pub incarnation: String,
    pub evidence_generation: String,
    pub through_seq: i64,
    pub head_revision: i64,
    pub record: AgentLogProjectionState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Evidence {
    #[serde(rename = "_id")]
    pub id: String,
    pub format_version: i64,
    pub agent_id: String,
    pub incarnation: String,
    pub evidence_generation: String,
    pub kind: String,
    pub entity_key: String,
    pub through_seq: i64,
    pub head_revision: i64,
    pub record: EvidenceRecord,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_sequence: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_ordinal: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(in crate::store) enum EvidenceRecord {
    TurnOutput {
        index: AgentTurnOutputIndex,
    },
    OutputChange {
        change: AgentOutputChange,
    },
    ToolOutput {
        tool: AgentToolOutputState,
    },
    Turn {
        facts: Box<AgentLogTurnFacts>,
    },
    Message {
        sequence: u64,
        turn_sequence: u64,
        role: AgentTranscriptRole,
    },
    Queued {
        sequence: u64,
        #[serde(deserialize_with = "super::required_nullable")]
        consumed_by: Option<u64>,
    },
    Approval {
        approval_id: String,
        #[serde(deserialize_with = "super::required_nullable")]
        request_sequence: Option<u64>,
        #[serde(deserialize_with = "super::required_nullable")]
        turn_sequence: Option<u64>,
        epoch: u64,
    },
    Operation {
        operation_sequence: u64,
        sequence: u64,
    },
}

pub(super) fn identity(head: &Head, kind: &str, key: &str) -> String {
    format!(
        "{}:{}:{}:{}:{}:{kind}:{}:{key}",
        head.agent_id.len(),
        head.agent_id,
        head.incarnation,
        head.evidence_generation.len(),
        head.evidence_generation,
        key.len()
    )
}

pub(in crate::store) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    head: &Head,
    session: &mut ClientSession,
) -> Result<AgentLogProjectionState, TransactionLogError> {
    read_budgeted(store, agent, head, session, None).await
}

pub(in crate::store) async fn read_budgeted(
    store: &MongoSandboxStore,
    agent: &Agent,
    head: &Head,
    session: &mut ClientSession,
    budget: Option<&mut super::capture::preview_budget::PreviewBudget>,
) -> Result<AgentLogProjectionState, TransactionLogError> {
    let projections = collection(store, PROJECTIONS);
    let row = if let Some(budget) = budget {
        budget
            .document(&projections, doc! {"_id":&head.agent_id}, None, session)
            .await?
            .ok_or_else(corrupt)?
    } else {
        // The head has already established this session's snapshot. Override the
        // collection's majority default, which cannot carry atClusterTime. A
        // transaction session has no snapshot_time and keeps its transaction concern.
        let snapshot = session.snapshot_time().is_some();
        crate::store::stale_turns::find_with_proof_budget(
            &projections,
            doc! {"_id":&head.agent_id},
            session,
            snapshot,
        )
        .await
        .map_err(TransactionLogError::Driver)?
        .ok_or_else(corrupt)?
    };
    validate_projection(row, agent, head)
}

pub(super) fn validate_projection(
    row: Document,
    agent: &Agent,
    head: &Head,
) -> Result<AgentLogProjectionState, TransactionLogError> {
    let projection: Projection = bson::deserialize_from_document(row).map_err(|_| corrupt())?;
    if projection.agent_id == head.agent_id && projection.incarnation == head.incarnation {
        super::telemetry::lag("projection", projection.through_seq, head.last_seq);
    }
    if projection.format_version != 1
        || projection.agent_id != head.agent_id
        || projection.incarnation != head.incarnation
        || projection.evidence_generation != head.evidence_generation
        || projection.through_seq != head.last_seq
        || projection.head_revision != head.revision
        || u64::try_from(head.last_seq).ok() != Some(projection.record.through_seq)
    {
        return Err(corrupt().into());
    }
    projection
        .record
        .validate_aggregate(agent)
        .map_err(SandboxStoreError::Corrupt)?;
    Ok(projection.record)
}

pub(in crate::store) async fn evidence(
    store: &MongoSandboxStore,
    head: &Head,
    kind: &str,
    key: &str,
    session: &mut ClientSession,
) -> Result<Option<EvidenceRecord>, TransactionLogError> {
    evidence_budgeted(store, head, kind, key, session, None).await
}

pub(in crate::store) async fn evidence_budgeted(
    store: &MongoSandboxStore,
    head: &Head,
    kind: &str,
    key: &str,
    session: &mut ClientSession,
    budget: Option<&mut super::capture::preview_budget::PreviewBudget>,
) -> Result<Option<EvidenceRecord>, TransactionLogError> {
    let evidence = collection(store, EVIDENCE);
    let row = if let Some(budget) = budget {
        budget.document(&evidence, doc! {"agent_id":&head.agent_id,"incarnation":&head.incarnation,"evidence_generation":&head.evidence_generation,"kind":kind,"entity_key":key}, None, session).await?
    } else {
        let snapshot = session.snapshot_time().is_some();
        crate::store::stale_turns::find_with_proof_budget(
            &evidence,
            doc! {"agent_id":&head.agent_id,"incarnation":&head.incarnation,
            "evidence_generation":&head.evidence_generation,"kind":kind,"entity_key":key},
            session,
            snapshot,
        )
        .await
        .map_err(TransactionLogError::Driver)?
    };
    let Some(row) = row else {
        return Ok(None);
    };
    read_evidence(head, kind, key, row)
        .map(Some)
        .map_err(Into::into)
}

pub(in crate::store) fn read_evidence(
    head: &Head,
    kind: &str,
    key: &str,
    row: Document,
) -> Result<EvidenceRecord, SandboxStoreError> {
    if kind == "output_change" {
        row.get_i64("turn_sequence").map_err(|_| corrupt())?;
        row.get_i64("change_ordinal").map_err(|_| corrupt())?;
    } else if row.contains_key("turn_sequence") || row.contains_key("change_ordinal") {
        return Err(corrupt());
    }
    let evidence: Evidence = bson::deserialize_from_document(row).map_err(|_| corrupt())?;
    if evidence.format_version != 1
        || evidence.id != identity(head, kind, key)
        || evidence.agent_id != head.agent_id
        || evidence.incarnation != head.incarnation
        || evidence.evidence_generation != head.evidence_generation
        || evidence.kind != kind
        || evidence.entity_key != key
        || evidence.through_seq < 1
        || evidence.through_seq > head.last_seq
        || evidence.head_revision < 1
        || evidence.head_revision > head.revision
    {
        return Err(corrupt());
    }
    validate_record(&evidence, evidence.through_seq as u64)?;
    Ok(evidence.record)
}

fn validate_record(evidence: &Evidence, through: u64) -> Result<(), SandboxStoreError> {
    if output::validate(evidence, through)? {
        return Ok(());
    }
    let covered = |seq| seq > 0 && seq <= through;
    let (kind, key, valid) = match &evidence.record {
        EvidenceRecord::TurnOutput { .. }
        | EvidenceRecord::OutputChange { .. }
        | EvidenceRecord::ToolOutput { .. } => return Err(corrupt()),
        EvidenceRecord::Turn { facts } => (
            "turn",
            facts.turn_sequence.to_string(),
            facts.validate(through).is_ok(),
        ),
        EvidenceRecord::Message {
            sequence,
            turn_sequence,
            role,
        } => (
            "message",
            sequence.to_string(),
            covered(*sequence)
                && covered(*turn_sequence)
                && (*role != AgentTranscriptRole::Agent || sequence > turn_sequence),
        ),
        EvidenceRecord::Queued {
            sequence,
            consumed_by,
        } => (
            "queued",
            sequence.to_string(),
            covered(*sequence) && consumed_by.is_none_or(|turn| covered(turn) && turn > *sequence),
        ),
        EvidenceRecord::Approval {
            approval_id,
            request_sequence,
            turn_sequence,
            epoch,
        } => (
            "approval",
            approval_id.clone(),
            *epoch <= through
                && request_sequence.is_none_or(|seq| covered(seq) && seq > *epoch)
                && turn_sequence.is_none_or(covered),
        ),
        EvidenceRecord::Operation {
            operation_sequence,
            sequence,
        } => (
            "operation",
            operation_sequence.to_string(),
            covered(*operation_sequence) && covered(*sequence) && sequence >= operation_sequence,
        ),
    };
    if !valid || kind != evidence.kind || key != evidence.entity_key {
        return Err(corrupt());
    }
    Ok(())
}

/// Planning is shared with bootstrap/rebuild; only the later gated publication
/// transaction may persist these rows for an activated Agent.
pub(in crate::store) fn plan(
    head: &Head,
    change: AgentLogEvidenceChange,
) -> Result<Document, SandboxStoreError> {
    use AgentLogEvidenceChange as Change;
    let (kind, key, record) = match change {
        Change::Turn(facts) => (
            "turn",
            facts.turn_sequence.to_string(),
            EvidenceRecord::Turn { facts },
        ),
        Change::Message {
            sequence,
            turn_sequence,
            role,
        } => (
            "message",
            sequence.to_string(),
            EvidenceRecord::Message {
                sequence,
                turn_sequence,
                role,
            },
        ),
        Change::Queued {
            sequence,
            consumed_by,
        } => (
            "queued",
            sequence.to_string(),
            EvidenceRecord::Queued {
                sequence,
                consumed_by,
            },
        ),
        Change::Approval {
            approval_id,
            request_sequence,
            turn_sequence,
            epoch,
        } => (
            "approval",
            approval_id.clone(),
            EvidenceRecord::Approval {
                approval_id,
                request_sequence,
                turn_sequence,
                epoch,
            },
        ),
        Change::Operation {
            operation_sequence,
            sequence,
        } => (
            "operation",
            operation_sequence.to_string(),
            EvidenceRecord::Operation {
                operation_sequence,
                sequence,
            },
        ),
    };
    let evidence = Evidence {
        id: identity(head, kind, &key),
        format_version: 1,
        agent_id: head.agent_id.clone(),
        incarnation: head.incarnation.clone(),
        evidence_generation: head.evidence_generation.clone(),
        kind: kind.into(),
        entity_key: key,
        through_seq: head.last_seq,
        head_revision: head.revision,
        record,
        turn_sequence: None,
        change_ordinal: None,
    };
    bson::serialize_to_document(&evidence).map_err(|_| corrupt())
}
