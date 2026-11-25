use super::ApplyError;
use layer2_core::sandbox::{
    store::{SandboxRuntimeLossRequest, SandboxStoreError},
    Agent, AgentWorkStep,
};
use mongodb::bson::{doc, Document};

use crate::store::{
    agent_control_receipt::RESTART_PREDECESSOR_FIELD,
    agent_control_transaction::{
        restart::ACTIVE_RESTART_FIELD, restart_cancel::TEARDOWN, restart_dispatch::DISPATCH_FIELD,
    },
};

/// Provider finality is the only alternative to a pinned runtime acknowledgement.
/// This remains part of the same transaction as the stopped Agent publication.
pub(super) fn restrict_to_retired_runtime(
    current: &Agent,
    request: &SandboxRuntimeLossRequest,
    filter: &mut Document,
) -> Result<Option<Document>, ApplyError> {
    if !request.native_retirement_finality
        || !matches!(
            current.work.step,
            AgentWorkStep::RestartConversation | AgentWorkStep::RestartTeardown
        )
    {
        return Ok(None);
    }
    let conflict = || ApplyError::Domain(SandboxStoreError::Conflict);
    let binding = request.expected_runtime.as_ref().ok_or_else(conflict)?;
    let boot = binding.boot_id.as_ref().ok_or_else(conflict)?;
    let generation = binding
        .provision_generation
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(conflict)?;
    filter.extend(doc! {
        "control_restart_active_v1.source.runtime.boot_id": boot,
        "control_restart_active_v1.source.runtime.runtime.generation": generation,
        "control_restart_active_v1.source.runtime.runtime.sandbox_id": current.sandbox_id.as_str(),
    });
    Ok(Some(doc! {
        ACTIVE_RESTART_FIELD: "", DISPATCH_FIELD: "", TEARDOWN: "", RESTART_PREDECESSOR_FIELD: "",
    }))
}
