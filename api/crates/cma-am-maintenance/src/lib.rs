//! MongoDB evidence records for the Sandbox maintenance observer and admission fence.

use std::collections::BTreeMap;

use mongodb::bson::{Bson, Document};

use layer2_core::sandbox::store::{runtime_activity::*, SandboxStoreError};
use layer2_core::SandboxId;

pub fn get<'a>(row: &'a Document, path: &str) -> Option<&'a Bson> {
    let (first, rest) = path.split_once('.').unwrap_or((path, ""));
    let value = row.get(first)?;
    if rest.is_empty() {
        Some(value)
    } else {
        get(value.as_document()?, rest)
    }
}

pub fn text<'a>(row: &'a Document, path: &str) -> Result<&'a str, SandboxStoreError> {
    get(row, path)
        .and_then(Bson::as_str)
        .filter(|value| {
            !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
        })
        .ok_or_else(invalid)
}

pub fn number(row: &Document, path: &str) -> Result<u64, SandboxStoreError> {
    match get(row, path) {
        Some(Bson::Int64(value)) => u64::try_from(*value).map_err(|_| invalid()),
        Some(Bson::Int32(value)) => u64::try_from(*value).map_err(|_| invalid()),
        _ => Err(invalid()),
    }
}

fn present(row: &Document, path: &str) -> bool {
    !matches!(get(row, path), None | Some(Bson::Null))
}

pub fn same(row: &Document, envelope: &str, record: &str) -> Result<(), SandboxStoreError> {
    if get(row, envelope).is_none() || get(row, envelope) != get(row, record) {
        return Err(invalid());
    }
    Ok(())
}

pub fn record(row: &Document, active: bool) -> Result<RuntimeActivityRecord, SandboxStoreError> {
    let metadata_sha256 = layer2_core::profiles::content_digest(row).map_err(|_| invalid())?;
    let mut counters = BTreeMap::new();
    for (field, counter) in [
        ("desktop_activity_fence", RuntimeActivityCounter::Desktop),
        (
            "agent_command_admission_fence",
            RuntimeActivityCounter::CommandAdmission,
        ),
        (
            "agent_conversation_admission_fence",
            RuntimeActivityCounter::ConversationAdmission,
        ),
        (
            "agent_conversation_reopen_fence",
            RuntimeActivityCounter::ConversationReopen,
        ),
        (
            "workspace_admission_fence",
            RuntimeActivityCounter::WorkspaceAdmission,
        ),
        (
            "owned_workspace_admission_fence",
            RuntimeActivityCounter::OwnedWorkspaceAdmission,
        ),
        (
            "sandbox_maintenance_fence",
            RuntimeActivityCounter::Maintenance,
        ),
        ("work_next_fence", RuntimeActivityCounter::WorkNextFence),
        ("last_order", RuntimeActivityCounter::CommandLastOrder),
        ("next_fence", RuntimeActivityCounter::CommandNextFence),
        (
            "projection_through",
            RuntimeActivityCounter::LogProjectionThrough,
        ),
    ] {
        counters.insert(
            counter,
            row.get(field).map(|_| number(row, field)).transpose()?,
        );
    }
    if row.contains_key("resource_share_guard") {
        number(row, "resource_share_guard")?;
    }
    let result = RuntimeActivityRecord {
        id: text(row, "_id")?.into(),
        metadata_sha256,
        revision: row
            .get("revision")
            .map(|_| number(row, "revision"))
            .transpose()?,
        last_seq: row
            .get("last_seq")
            .map(|_| number(row, "last_seq"))
            .transpose()?,
        counters,
        active,
    };
    result.validate()?;
    Ok(result)
}

pub fn aggregate(
    row: &Document,
    id: &SandboxId,
    agent: bool,
) -> Result<RuntimeActivityRecord, SandboxStoreError> {
    if agent {
        cma_am_agent_log::require_agent_version(row).map_err(|_| invalid())?;
    } else {
        cma_am_sandbox_state::work_schema::require(row).map_err(|_| invalid())?;
    }
    for (envelope, field) in [
        ("_id", "record.id"),
        ("revision", "record.revision"),
        ("status", "record.status"),
    ] {
        same(row, envelope, field)?;
    }
    if row.contains_key("sandbox_id") {
        same(row, "sandbox_id", "record.sandbox_id")?;
        if text(row, "sandbox_id")? != id.as_str() {
            return Err(invalid());
        }
    } else if text(row, "_id")? != id.as_str() {
        return Err(invalid());
    }
    let step = get(row, "record.work.step").ok_or_else(invalid)?;
    if agent {
        let _: layer2_core::sandbox::AgentWorkStep =
            mongodb::bson::deserialize_from_bson(step.clone()).map_err(|_| invalid())?;
    } else if row.contains_key("sandbox_id") {
        let _: layer2_core::sandbox::WorkspaceWorkStep =
            mongodb::bson::deserialize_from_bson(step.clone()).map_err(|_| invalid())?;
    } else {
        let _: layer2_core::sandbox::SandboxWorkStep =
            mongodb::bson::deserialize_from_bson(step.clone()).map_err(|_| invalid())?;
    }
    if layer2_core::seconds_from_rfc3339(text(row, "record.created_at")?).is_none() {
        return Err(invalid());
    }
    if number(row, "record.work.next_fence")? == 0 {
        return Err(invalid());
    }
    let expiry = get(row, "record.work.lease.expires_at").unwrap_or(&Bson::Null);
    if get(row, "work_lease_expires_at") != Some(expiry)
        || (present(row, "record.work.lease")
            && (layer2_core::seconds_from_rfc3339(text(row, "record.work.lease.expires_at")?)
                .is_none()
                || number(row, "record.work.lease.fence")? == 0
                || number(row, "record.work.lease.fence")?
                    >= number(row, "record.work.next_fence")?))
    {
        return Err(invalid());
    }
    let step = step
        .as_str()
        .or_else(|| step.as_document().and_then(|row| row.get_str("kind").ok()))
        .ok_or_else(invalid)?;
    if row.get_bool("work_pending").map_err(|_| invalid())? != (step != "none") {
        return Err(invalid());
    }
    let mut active = !matches!(step, "none" | "wait_for_credential_rotation")
        || present(row, "record.work.lease")
        || (present(row, "record.lifecycle.operation") && !parked_update(row, step)?);
    if !agent {
        let status = get(row, "status").ok_or_else(invalid)?.clone();
        if row.contains_key("sandbox_id") {
            let _: layer2_core::sandbox::WorkspaceStatus =
                mongodb::bson::deserialize_from_bson(status).map_err(|_| invalid())?;
            same(row, "next_worktree_number", "record.next_worktree_number")?;
        } else {
            let _: layer2_core::sandbox::SandboxStatus =
                mongodb::bson::deserialize_from_bson(status).map_err(|_| invalid())?;
        }
    }
    if agent {
        for (envelope, field) in [
            ("workspace_id", "record.workspace_id"),
            ("worktree_id", "record.worktree_id"),
            ("creation_key", "record.creation_key"),
            ("creation_fingerprint", "record.creation_fingerprint"),
            ("last_seq", "record.conversation.last_seq"),
            ("work_next_fence", "record.work.next_fence"),
        ] {
            same(row, envelope, field)?;
        }
        for (envelope, field) in [
            ("work_lease_holder", "record.work.lease.holder"),
            ("work_lease_fence", "record.work.lease.fence"),
        ] {
            if get(row, envelope) != Some(get(row, field).unwrap_or(&Bson::Null)) {
                return Err(invalid());
            }
        }
        let queued = get(row, "record.conversation.queued_messages")
            .and_then(Bson::as_array)
            .ok_or_else(invalid)?;
        let operation = if present(row, "record.conversation.operation") {
            let state = text(row, "record.conversation.operation.state")?;
            if !matches!(
                state,
                "dispatch_fenced"
                    | "write_confirmed"
                    | "execution_observed"
                    | "pre_write_failed"
                    | "recovery_required"
                    | "runtime_lost"
            ) {
                return Err(invalid());
            }
            matches!(state, "dispatch_fenced" | "recovery_required")
        } else {
            false
        };
        let status = text(row, "status")?;
        if !matches!(
            status,
            "starting"
                | "ready"
                | "working"
                | "waiting_approval"
                | "paused"
                | "stopped"
                | "failed"
                | "deleted"
        ) {
            return Err(invalid());
        }
        let cleanup = get(row, "record.work.cleanup_debt")
            .and_then(Bson::as_array)
            .ok_or_else(invalid)?;
        let deleted_invalid = status == "deleted"
            && (!cleanup.is_empty()
                || layer2_core::seconds_from_rfc3339(text(row, "record.deleted_at")?).is_none());
        active |= !queued.is_empty()
            || (status != "deleted" && present(row, "record.conversation.active_turn"))
            || operation
            || deleted_invalid
            || present(row, "record.requested_stop_reason")
            || matches!(status, "starting" | "working" | "waiting_approval");
    }
    record(row, active)
}

pub fn command(row: &Document) -> Result<RuntimeActivityRecord, SandboxStoreError> {
    cma_am_mongo_primitives::require_version(row, 1).map_err(|_| invalid())?;
    for (envelope, field) in [
        ("_id", "record.receipt.envelope.command_id"),
        ("agent_id", "record.receipt.envelope.agent_id"),
        ("agent_order", "record.receipt.agent_order"),
        ("revision", "record.revision"),
        ("status", "record.status"),
    ] {
        same(row, envelope, field)?;
    }
    let status = text(row, "status")?;
    if !matches!(
        status,
        "queued" | "in_progress" | "waiting_action" | "completed" | "failed" | "cancelled"
    ) {
        return Err(invalid());
    }
    record(
        row,
        matches!(status, "queued" | "in_progress" | "waiting_action")
            || present(row, "record.work.lease"),
    )
}

fn parked_update(row: &Document, step: &str) -> Result<bool, SandboxStoreError> {
    if step != "none"
        || present(row, "record.work.lease")
        || !present(row, "record.lifecycle.runtime_update.request.failure")
        || !present(row, "record.lifecycle.runtime_update.request.started_at")
        || present(row, "record.lifecycle.runtime_update.request.completed_at")
    {
        return Ok(false);
    }
    let target = text(
        row,
        "record.lifecycle.runtime_update.request.target_version",
    )?;
    let purpose = text(row, "record.lifecycle.operation.purpose")?;
    let phase = get(row, "record.lifecycle.operation.phase")
        .ok_or_else(invalid)?
        .clone();
    if layer2_core::seconds_from_rfc3339(text(
        row,
        "record.lifecycle.runtime_update.request.started_at",
    )?)
    .is_none()
    {
        return Err(invalid());
    }
    Ok(match purpose {
        "image_rotation" => {
            let _: layer2_core::sandbox::SandboxImageRotationPhase =
                mongodb::bson::deserialize_from_bson(phase).map_err(|_| invalid())?;
            text(row, "record.lifecycle.operation.target_runtime_version")? == target
        }
        "revive" => {
            let _: layer2_core::sandbox::SandboxRevivePhase =
                mongodb::bson::deserialize_from_bson(phase).map_err(|_| invalid())?;
            true
        }
        _ => false,
    })
}

fn invalid() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("runtime activity source is incomplete or changed")
}
