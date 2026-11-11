//! Pure, bounded publication delta. Coalesce evidence keys after sequential
//! folding so repeated changes in one batch never issue competing upserts.
use layer2_core::sandbox::{
    store::SandboxStoreError, Agent, AgentLogEntry, AgentLogProjectionState,
};
use mongodb::bson::{self, Document};
use std::collections::BTreeMap;

use super::writer_plan::SourceFormat;
use super::{
    corrupt, digest,
    projection::{self, Projection},
    Head,
};
use crate::store::agent_log_integrity::ensure_append_batch_size;

const MAX_PROJECTION_BYTES: usize = 256 * 1024;
const MAX_EVIDENCE_UPSERTS: usize = 64 * 5;

pub(super) struct ProjectionDelta {
    pub expected_revision: i64,
    pub expected_through: i64,
    pub projection: Document,
    pub evidence: Vec<Document>,
}

pub(super) fn plan(
    current: &Agent,
    next: &Agent,
    before: &Head,
    after: &Head,
    state: &AgentLogProjectionState,
    logs: &[AgentLogEntry],
    source: SourceFormat,
) -> Result<ProjectionDelta, SandboxStoreError> {
    ensure_append_batch_size(logs.len())?;
    before.validate()?;
    after.validate()?;
    let revision_valid = match source {
        SourceFormat::Legacy => before.active_ordinal == 0 && after.revision == 1,
        SourceFormat::Segmented => before.revision.checked_add(1) == Some(after.revision),
    };
    if !revision_valid
        || before.agent_id != current.id.as_str()
        || current.id != next.id
        || before.incarnation != digest::incarnation(current)
        || after.agent_id != before.agent_id
        || after.incarnation != before.incarnation
        || after.evidence_generation != before.evidence_generation
        || after.last_seq.checked_sub(before.last_seq) != Some(logs.len() as i64)
        || u64::try_from(before.last_seq).ok() != Some(current.conversation.last_seq)
        || u64::try_from(after.last_seq).ok() != Some(next.conversation.last_seq)
    {
        return Err(corrupt());
    }
    state
        .validate_aggregate(current)
        .map_err(SandboxStoreError::Corrupt)?;
    let mut state = state.clone();
    let mut evidence = BTreeMap::new();
    for entry in logs {
        let queued = AgentLogProjectionState::queued_owner(entry);
        for change in state
            .observe(next, entry, queued)
            .map_err(SandboxStoreError::Corrupt)?
        {
            let row = projection::plan(after, change)?;
            let key = row.get_str("_id").map_err(|_| corrupt())?.to_owned();
            evidence.insert(key, row);
        }
    }
    state
        .validate_aggregate(next)
        .map_err(SandboxStoreError::Corrupt)?;
    for row in evidence.values() {
        projection::read_evidence(
            after,
            row.get_str("kind").map_err(|_| corrupt())?,
            row.get_str("entity_key").map_err(|_| corrupt())?,
            row.clone(),
        )?;
    }
    let projection = bson::serialize_to_document(&Projection {
        agent_id: after.agent_id.clone(),
        format_version: after.projection_format,
        incarnation: after.incarnation.clone(),
        evidence_generation: after.evidence_generation.clone(),
        through_seq: after.last_seq,
        head_revision: after.revision,
        record: state,
    })
    .map_err(|_| corrupt())?;
    if bson::serialize_to_vec(&projection)
        .map_err(|_| corrupt())?
        .len()
        > MAX_PROJECTION_BYTES
        || evidence.len() > MAX_EVIDENCE_UPSERTS
        || evidence.values().any(|row| {
            bson::serialize_to_vec(row).map_or(true, |bytes| bytes.len() > MAX_PROJECTION_BYTES)
        })
    {
        return Err(corrupt());
    }
    Ok(ProjectionDelta {
        expected_revision: before.revision,
        expected_through: before.last_seq,
        projection,
        evidence: evidence.into_values().collect(),
    })
}

#[cfg(test)]
#[path = "writer_projection_tests.rs"]
pub(super) mod tests;
