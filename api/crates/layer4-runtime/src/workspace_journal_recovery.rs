use super::*;

/// Read-only proof of the one recoverable original outcome. Recovery never
/// rewrites, completes or replaces this original admission journal.
pub(crate) fn indeterminate_prepare_digest(
    config: &crate::config::RuntimeConfig,
    request: &layer3_adapter::WorkspaceRequest,
) -> Result<String> {
    use sha2::{Digest, Sha256};
    require_present(
        &config.state_root,
        &config.home,
        config.allow_local_git,
        &config.sandbox_id,
        &request.workspace_id,
    )?;
    let bytes = io::read_bytes(&io::journal_path(&config.state_root, &request.operation_id))?
        .ok_or_else(|| corrupt("original Workspace preparation journal is absent"))?;
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|_| corrupt("original Workspace journal is invalid"))?;
    validate_stored(&record, &config.home, config.allow_local_git)?;
    if record.sandbox_id != config.sandbox_id
        || record.workspace_id != request.workspace_id
        || record.operation_id != request.operation_id
        || record.request_sha256 != request.request_sha256
        || record.request
            != serde_json::to_value(request).map_err(|_| corrupt("invalid original request"))?
        || record.kind != "prepare"
        || record.status != Status::Indeterminate
        || record
            .failure
            .as_ref()
            .is_none_or(|failure| failure.code != "workspace_prepare_outcome_unknown")
    {
        return Err(RuntimeError::rejected(
            "workspace_recovery_ineligible",
            "only the exact indeterminate original Workspace preparation can be recovered",
        ));
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
