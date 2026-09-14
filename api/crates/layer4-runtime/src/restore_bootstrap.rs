use layer3_adapter::{RestoreBootstrapReceipt, RestoreBootstrapRequest};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::install_state::{self, InstallRecord};

pub(crate) fn admit(
    config: &RuntimeConfig,
    request: &RestoreBootstrapRequest,
) -> Result<RestoreBootstrapReceipt> {
    validate(config, request)?;
    let candidate = RestoreBootstrapReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        restore_request_sha256: request.checkpoint.computed_sha256(),
        installation_inherited: true,
    };
    if let Some(record) = install_state::load(config)? {
        return replay(&candidate, record);
    }
    if install_state::write_initial(config, &InstallRecord::inherited(config, candidate.clone()))? {
        return Ok(candidate);
    }
    replay(
        &candidate,
        install_state::load(config)?.ok_or_else(|| {
            RuntimeError::unavailable(
                "install_state_unavailable",
                "installation state disappeared during restore bootstrap admission",
            )
        })?,
    )
}

fn replay(
    candidate: &RestoreBootstrapReceipt,
    record: InstallRecord,
) -> Result<RestoreBootstrapReceipt> {
    match record.bootstrap_receipt() {
        Some(existing) if existing == candidate => Ok(existing.clone()),
        Some(_) => Err(RuntimeError::rejected(
            "restore_bootstrap_conflict",
            "restore bootstrap does not match the admitted checkpoint identity",
        )),
        None => Err(RuntimeError::rejected(
            "install_mode_conflict",
            "ordinary installation was already admitted for this runtime",
        )),
    }
}

fn validate(config: &RuntimeConfig, request: &RestoreBootstrapRequest) -> Result<()> {
    if request.operation_id.trim().is_empty()
        || request.operation_id.len() > 256
        || request.operation_id.chars().any(char::is_control)
    {
        return Err(RuntimeError::rejected(
            "restore_bootstrap_operation_invalid",
            "restore bootstrap operation id is empty, too large, or contains control characters",
        ));
    }
    if request.sandbox_id != config.sandbox_id {
        return Err(RuntimeError::rejected(
            "restore_bootstrap_sandbox_mismatch",
            "restore bootstrap names another sandbox",
        ));
    }
    if !request.has_valid_sha256() || !request.checkpoint.has_valid_reference() {
        return Err(RuntimeError::rejected(
            "restore_bootstrap_identity_invalid",
            "restore bootstrap request or checkpoint identity is invalid",
        ));
    }
    Ok(())
}
