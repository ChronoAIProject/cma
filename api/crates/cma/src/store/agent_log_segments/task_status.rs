//! Task-status source evidence remains bounded across completed and active turns.
use super::{
    consumers::{points, semantic},
    corrupt, projection,
};
use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::sandbox::{store::SandboxStoreError, Agent, AgentLogEntry, AgentLogKind};
use mongodb::ClientSession;

type StatusEvidence = (Option<AgentLogEntry>, Option<AgentLogEntry>);

pub(in crate::store) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Option<StatusEvidence>, SandboxStoreError> {
    let Some((head, state)) = semantic(store, agent, session, true)
        .await
        .map_err(super::approvals::store_error)?
    else {
        return Ok(None);
    };
    let latest_sequence = state.latest_task_status.ok_or_else(corrupt)?;
    let facts = match state.latest_turn {
        None => None,
        Some(sequence) => {
            match projection::evidence(store, &head, "turn", &sequence.to_string(), session)
                .await
                .map_err(super::approvals::store_error)?
            {
                Some(projection::EvidenceRecord::Turn { facts }) => Some(facts),
                _ => return Err(corrupt()),
            }
        }
    };
    let gap_sequence = facts
        .as_ref()
        .filter(|facts| facts.end_sequence == Some(latest_sequence))
        .and_then(|facts| facts.first_gap_sequence);
    let mut references = vec![latest_sequence];
    references.extend(gap_sequence);
    references.sort_unstable();
    let source = points(store, &head, &references, session, true)
        .await
        .map_err(super::approvals::store_error)?;
    let latest = source
        .iter()
        .find(|entry| entry.seq == latest_sequence)
        .cloned()
        .ok_or_else(corrupt)?;
    if !layer2_core::sandbox::AgentTaskStatus::relevant(&latest.kind) {
        return Err(corrupt());
    }
    let gap = gap_sequence
        .map(|seq| {
            source
                .iter()
                .find(|entry| entry.seq == seq)
                .cloned()
                .ok_or_else(corrupt)
        })
        .transpose()?;
    if gap.as_ref().is_some_and(
        |entry| !matches!(entry.kind, AgentLogKind::ConversationGap { frames } if frames > 0),
    ) {
        return Err(corrupt());
    }
    Ok(Some((Some(latest), gap)))
}
