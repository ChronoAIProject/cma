//! Keep the Stop consumed by Agent restart separate from current control work.
use super::{
    corrupt, receipt_belongs_to_agent, validate_receipt, Agent, AgentControlStoreError,
    AgentStatus, ControlReceipt, ControlRequest, Document,
};
use layer2_core::sandbox::AgentStopReason;
use layer3_adapter::{AgentCloseRequest, AgentProcessAction, AgentSlot};

pub(in crate::store) const FIELD: &str = "control_restart_predecessor_v1";

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct RestartPredecessor {
    restart_revision: i64,
    conversation_id: String,
    provision_generation: u64,
    closed_incarnation: u64,
    restart_incarnation: u64,
    close_operation_id: String,
    close_request_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stopped_at: Option<String>,
    stop: ControlReceipt,
}

pub(super) fn encode_predecessor(
    stop: &ControlReceipt,
    stopped_at: &str,
    acceptance: &layer2_core::sandbox::store::AgentRestartAcceptance,
) -> Result<Document, AgentControlStoreError> {
    let predecessor = RestartPredecessor {
        restart_revision: i64::try_from(acceptance.revision)
            .map_err(|_| AgentControlStoreError::Conflict)?,
        conversation_id: acceptance.source.conversation_id.clone(),
        provision_generation: acceptance.source.runtime.runtime.generation,
        closed_incarnation: acceptance
            .next_incarnation
            .checked_sub(1)
            .ok_or(AgentControlStoreError::Conflict)?,
        restart_incarnation: acceptance.next_incarnation,
        close_operation_id: acceptance.source.predecessor_close.operation_id.clone(),
        close_request_sha256: acceptance.source.predecessor_close.request_sha256.clone(),
        stopped_at: Some(stopped_at.to_owned()),
        stop: stop.clone(),
    };
    Ok(mongodb::bson::doc! {
        (FIELD): super::record_bson(&predecessor)
            .map_err(|error| AgentControlStoreError::Corrupt(error.to_string()))?,
    })
}

pub(super) fn matches_acceptance(
    row: &Document,
    acceptance: &layer2_core::sandbox::store::AgentRestartAcceptance,
) -> bool {
    let Some(predecessor) = row.get(FIELD).and_then(|value| {
        mongodb::bson::deserialize_from_bson::<RestartPredecessor>(value.clone()).ok()
    }) else {
        return false;
    };
    u64::try_from(predecessor.restart_revision) == Ok(acceptance.revision)
        && predecessor.conversation_id == acceptance.source.conversation_id
        && predecessor.provision_generation == acceptance.source.runtime.runtime.generation
        && predecessor.restart_incarnation == acceptance.next_incarnation
        && predecessor.close_operation_id == acceptance.source.predecessor_close.operation_id
        && predecessor.close_request_sha256 == acceptance.source.predecessor_close.request_sha256
        && predecessor.stop.operation_id == acceptance.predecessor_stop_operation
        && predecessor.stopped_at.as_deref().is_none_or(|at| {
            use layer2_core::seconds_from_rfc3339;
            seconds_from_rfc3339(at)
                .zip(seconds_from_rfc3339(
                    &acceptance.source.predecessor_close.completed_at,
                ))
                .zip(seconds_from_rfc3339(&acceptance.accepted_at))
                .is_some_and(|((stopped, closed), accepted)| {
                    closed <= stopped && stopped <= accepted
                })
        })
}

pub(in crate::store) fn restore_owner_stop(
    row: &Document,
) -> Result<(String, mongodb::bson::Bson), AgentControlStoreError> {
    let invalid = || corrupt("stored Agent restart predecessor cannot restore the owner Stop");
    let predecessor = row.get_document(FIELD).map_err(|_| invalid())?;
    let stopped_at = predecessor.get_str("stopped_at").map_err(|_| invalid())?;
    let receipt = predecessor.get("stop").cloned().ok_or_else(invalid)?;
    Ok((stopped_at.to_owned(), receipt))
}

pub(super) fn validate_predecessor(
    row: &Document,
    agent: &Agent,
    stop: Option<&ControlReceipt>,
    delete: Option<&ControlReceipt>,
) -> Result<(), AgentControlStoreError> {
    let Some(value) = row.get(FIELD) else {
        return Ok(());
    };
    let invalid = || corrupt("stored Agent restart predecessor is inconsistent");
    let predecessor: RestartPredecessor =
        mongodb::bson::deserialize_from_bson(value.clone()).map_err(|_| invalid())?;
    validate_receipt(&predecessor.stop, FIELD, true)?;
    let restart_revision = u64::try_from(predecessor.restart_revision).map_err(|_| invalid())?;
    let close = AgentCloseRequest::new(
        AgentProcessAction::Close.operation_id(agent.id.as_str(), predecessor.closed_incarnation),
        AgentSlot::new(agent.id.as_str()),
    );
    if !matches!(&predecessor.stop.request, ControlRequest::Stop(request) if request.reason == AgentStopReason::Owner)
        || !receipt_belongs_to_agent(&predecessor.stop, agent)
        || predecessor.restart_revision <= predecessor.stop.scheduled_revision
        || restart_revision > agent.revision
        || (restart_revision == agent.revision && agent.status != AgentStatus::Starting)
        || agent.conversation.conversation_id.as_ref() != Some(&predecessor.conversation_id)
        || predecessor.conversation_id.trim().is_empty()
        || predecessor.provision_generation == 0
        || predecessor.closed_incarnation == 0
        || predecessor.closed_incarnation.checked_add(1) != Some(predecessor.restart_incarnation)
        || predecessor.close_operation_id != close.operation_id
        || predecessor.close_request_sha256 != close.request_sha256
        || stop.into_iter().chain(delete).any(|receipt| {
            receipt.scheduled_revision <= predecessor.restart_revision
                || receipt.operation_id == predecessor.stop.operation_id
        })
    {
        return Err(invalid());
    }
    Ok(())
}
