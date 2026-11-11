//! Durable identity for one Mongo agent-control invocation.

use layer2_core::sandbox::store::{
    AgentControlResult, AgentControlStoreError, AgentControlTarget, AgentDeleteRequest,
    AgentStopRequest,
};
use layer2_core::sandbox::{Agent, AgentStatus};
use mongodb::bson::Document;
use serde::{Deserialize, Serialize};

use super::agent_control_store::{is_delete_work, is_stop_work, plan_delete, plan_stop};
use super::sandbox_mongo::{record_bson, DOCUMENT_SCHEMA_VERSION};

pub(super) const STOP_RECEIPT_FIELD: &str = "control_stop_receipt";
pub(super) const DELETE_RECEIPT_FIELD: &str = "control_delete_receipt";
const CONTROL_OPERATION_PREFIX: &str = "ctl_";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "request",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(super) enum ControlRequest {
    Stop(AgentStopRequest),
    Delete(AgentDeleteRequest),
}

impl ControlRequest {
    pub(super) fn target(&self) -> &AgentControlTarget {
        match self {
            Self::Stop(request) => &request.target,
            Self::Delete(request) => &request.target,
        }
    }

    pub(super) fn plan(
        &self,
        current: Agent,
    ) -> Result<AgentControlResult, AgentControlStoreError> {
        match self {
            Self::Stop(request) => plan_stop(current, request),
            Self::Delete(request) => plan_delete(current, request),
        }
    }

    pub(super) fn durable_replay(&self, current: &Agent, scheduled: &Agent) -> bool {
        match self {
            Self::Stop(request) => stop_workflow_descendant(current, &request.reason),
            Self::Delete(request) => {
                delete_workflow_descendant(current, request.target.expected_status)
                    && current.stop_reason == scheduled.stop_reason
                    && current.stopped_at == scheduled.stopped_at
                    && current.failure == scheduled.failure
            }
        }
    }

    fn receipt_field(&self) -> &'static str {
        match self {
            Self::Stop(_) => STOP_RECEIPT_FIELD,
            Self::Delete(_) => DELETE_RECEIPT_FIELD,
        }
    }
}

#[derive(Clone)]
pub(super) struct ControlOperation {
    pub(super) request: ControlRequest,
    operation_id: String,
}

impl ControlOperation {
    pub(super) fn new(request: ControlRequest) -> Self {
        Self {
            request,
            operation_id: format!(
                "{CONTROL_OPERATION_PREFIX}{}",
                uuid::Uuid::new_v4().simple()
            ),
        }
    }

    pub(super) fn id(&self) -> &str {
        &self.operation_id
    }

    pub(super) fn receipt_field(&self) -> &'static str {
        self.request.receipt_field()
    }

    pub(super) fn receipt_update(
        &self,
        scheduled: &Agent,
    ) -> Result<Document, AgentControlStoreError> {
        let scheduled_revision =
            i64::try_from(scheduled.revision).map_err(|_| AgentControlStoreError::Conflict)?;
        let receipt = ControlReceipt {
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            operation_id: self.operation_id.clone(),
            scheduled_revision,
            request: self.request.clone(),
        };
        let receipt = record_bson(&receipt)
            .map_err(|detail| AgentControlStoreError::Corrupt(detail.to_string()))?;
        let mut update = Document::new();
        update.insert(self.request.receipt_field(), receipt);
        Ok(update)
    }

    pub(super) fn was_recorded(&self, receipts: &StoredControlReceipts, scheduled: &Agent) -> bool {
        let receipt = match &self.request {
            ControlRequest::Stop(_) => receipts.stop.as_ref(),
            ControlRequest::Delete(_) => receipts.delete.as_ref(),
        };
        receipt.is_some_and(|receipt| {
            receipt.operation_id == self.operation_id
                && receipt.request == self.request
                && u64::try_from(receipt.scheduled_revision).ok() == Some(scheduled.revision)
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlReceipt {
    document_schema_version: i64,
    operation_id: String,
    scheduled_revision: i64,
    request: ControlRequest,
}

pub(super) struct StoredControlReceipts {
    stop: Option<ControlReceipt>,
    delete: Option<ControlReceipt>,
}

pub(super) fn read_control_receipts(
    document: &Document,
    agent: &Agent,
) -> Result<StoredControlReceipts, AgentControlStoreError> {
    let stop = read_receipt(document, STOP_RECEIPT_FIELD, true)?;
    let delete = read_receipt(document, DELETE_RECEIPT_FIELD, false)?;
    if stop
        .iter()
        .chain(delete.iter())
        .any(|receipt| !receipt_belongs_to_agent(receipt, agent))
    {
        return Err(corrupt(
            "stored agent control receipt does not match its agent",
        ));
    }
    if stop.as_ref().is_some_and(|receipt| {
        let ControlRequest::Stop(request) = &receipt.request else {
            return true;
        };
        !stop_workflow_descendant(agent, &request.reason)
    }) || delete.as_ref().is_some_and(|receipt| {
        let ControlRequest::Delete(request) = &receipt.request else {
            return true;
        };
        !delete_workflow_descendant(agent, request.target.expected_status)
    }) {
        return Err(corrupt(
            "stored agent control receipt disagrees with the current workflow",
        ));
    }
    if matches!((&stop, &delete), (Some(stop), Some(delete)) if
        delete.scheduled_revision <= stop.scheduled_revision
            || delete.operation_id == stop.operation_id)
    {
        return Err(corrupt("stored agent control receipts are out of order"));
    }
    Ok(StoredControlReceipts { stop, delete })
}

fn receipt_belongs_to_agent(receipt: &ControlReceipt, agent: &Agent) -> bool {
    let target = receipt.request.target();
    target.agent_id == agent.id
        && target.worktree_id == agent.worktree_id
        && target.workspace_id == agent.workspace_id
        && target.sandbox_id == agent.sandbox_id
        && target.owner == agent.owner
        && u64::try_from(receipt.scheduled_revision)
            .is_ok_and(|revision| revision <= agent.revision)
}

fn read_receipt(
    document: &Document,
    field: &str,
    stop: bool,
) -> Result<Option<ControlReceipt>, AgentControlStoreError> {
    let Some(value) = document.get(field) else {
        return Ok(None);
    };
    let receipt: ControlReceipt = mongodb::bson::deserialize_from_bson(value.clone())
        .map_err(|error| corrupt(&format!("stored {field} is invalid: {error}")))?;
    let expected_revision = receipt
        .request
        .target()
        .expected_revision
        .checked_add(1)
        .and_then(|revision| i64::try_from(revision).ok());
    let request_kind_matches = matches!(receipt.request, ControlRequest::Stop(_)) == stop;
    let request_status_matches = match &receipt.request {
        ControlRequest::Stop(request) => request.target.expected_status.is_live(),
        ControlRequest::Delete(request) => matches!(
            request.target.expected_status,
            AgentStatus::Stopped | AgentStatus::Failed
        ),
    };
    if receipt.document_schema_version != DOCUMENT_SCHEMA_VERSION
        || !operation_id_is_valid(&receipt.operation_id)
        || expected_revision != Some(receipt.scheduled_revision)
        || !request_kind_matches
        || !request_status_matches
    {
        return Err(corrupt(&format!("stored {field} is inconsistent")));
    }
    Ok(Some(receipt))
}

fn operation_id_is_valid(value: &str) -> bool {
    value
        .strip_prefix(CONTROL_OPERATION_PREFIX)
        .is_some_and(|suffix| {
            suffix.len() == 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

fn stop_workflow_descendant(
    current: &Agent,
    reason: &layer2_core::sandbox::AgentStopReason,
) -> bool {
    if current.status.is_live() {
        return current.requested_stop_reason.as_ref() == Some(reason)
            && current.stop_reason.is_none()
            && current.stopped_at.is_none()
            && current.deleted_at.is_none()
            && is_stop_work(current.work.step);
    }
    if current.status == AgentStatus::Stopped {
        return current.requested_stop_reason.is_none()
            && current.stop_reason.as_ref() == Some(reason)
            && current.stopped_at.is_some()
            && current.deleted_at.is_none()
            && (current.work.step == layer2_core::sandbox::AgentWorkStep::None
                || is_delete_work(current.work.step));
    }
    current.status == AgentStatus::Deleted
        && current.requested_stop_reason.is_none()
        && current.stop_reason.as_ref() == Some(reason)
        && current.stopped_at.is_some()
        && current.deleted_at.is_some()
        && (current.work.step == layer2_core::sandbox::AgentWorkStep::None
            || is_delete_work(current.work.step))
}

fn delete_workflow_descendant(current: &Agent, scheduled_from: AgentStatus) -> bool {
    if current.status == scheduled_from
        && matches!(scheduled_from, AgentStatus::Stopped | AgentStatus::Failed)
    {
        return current.requested_stop_reason.is_none()
            && current.deleted_at.is_none()
            && is_delete_work(current.work.step);
    }
    current.status == AgentStatus::Deleted
        && current.requested_stop_reason.is_none()
        && current.deleted_at.is_some()
        && (current.work.step == layer2_core::sandbox::AgentWorkStep::None
            || is_delete_work(current.work.step))
}

fn corrupt(detail: &str) -> AgentControlStoreError {
    AgentControlStoreError::Corrupt(detail.into())
}
