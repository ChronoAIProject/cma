//! Build creation metadata from the already validated immutable creation receipt.
use super::*;
use crate::store::agent_log_integrity::empty_summary;

pub(super) fn initial_integrity(
    creation: &layer2_core::sandbox::store::AgentCreation,
) -> Result<crate::store::agent_log_integrity::PreparedAppend, SandboxStoreError> {
    let log_summary = empty_summary();

    let semantic = SemanticCheckpoint::empty()
        .after_append(std::slice::from_ref(&creation.initial_log), &creation.agent)?;
    let integrity = prepare_append(
        &creation.agent,
        &log_summary,
        std::slice::from_ref(&creation.initial_log),
        creation.agent.revision,
        semantic,
        AppendCapacity::Normal,
    )?;
    Ok(integrity)
}
