use std::sync::atomic::{AtomicBool, Ordering};

use super::super::deployment::provider_request_identity;
use super::state::{digest, refused, successful_commands};
use super::MemoryRuntime;
use crate::wire::{CredentialReceipt, ResponseFrame, WireOperation};
use crate::{
    CommandReceipt, CredentialFrame, InstallRequest, RestoreBootstrapReceipt,
    RestoreBootstrapRequest,
};

pub(super) fn install(
    runtime: &mut MemoryRuntime,
    corrupt_receipt: &AtomicBool,
    request: InstallRequest,
    operation: WireOperation,
) -> crate::Result<ResponseFrame> {
    if request.operation_id.trim().is_empty() || !request.has_valid_sha256() {
        return refused(operation, "install_request_invalid");
    }
    if runtime.restore_bootstrap.is_some() {
        return refused(operation, "install_mode_conflict");
    }
    let identity = (request.operation_id.clone(), request.request_sha256.clone());
    if let Some((completed, receipt)) = &runtime.install {
        return if completed == &identity {
            Ok(ResponseFrame::Install(receipt.clone()))
        } else {
            refused(operation, "install_replay_mismatch")
        };
    }
    let receipt = crate::InstallReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256,
        commands: successful_commands(&request.commands),
    };
    runtime.install = Some((identity, receipt.clone()));
    let mut response = receipt;
    if corrupt_receipt.swap(false, Ordering::Relaxed) {
        if let Some(first) = response.commands.first_mut() {
            first.index = u32::MAX;
        } else {
            response.commands.push(CommandReceipt {
                index: 0,
                exit_code: 0,
                stdout_sha256: digest(&[]),
                stderr_sha256: digest(&[]),
            });
        }
    }
    Ok(ResponseFrame::Install(response))
}

pub(super) fn restore(
    runtime: &mut MemoryRuntime,
    request: RestoreBootstrapRequest,
    operation: WireOperation,
) -> crate::Result<ResponseFrame> {
    let identity = provider_request_identity(&runtime.request)?;
    if request.operation_id.trim().is_empty()
        || request.sandbox_id != identity.sandbox_id
        || !request.has_valid_sha256()
        || !request.checkpoint.has_valid_reference()
    {
        return refused(operation, "restore_bootstrap_invalid");
    }
    if runtime.install.is_some() {
        return refused(operation, "install_mode_conflict");
    }
    let receipt = RestoreBootstrapReceipt {
        operation_id: request.operation_id,
        request_sha256: request.request_sha256,
        restore_request_sha256: request.checkpoint.computed_sha256(),
        installation_inherited: true,
    };
    if let Some(existing) = &runtime.restore_bootstrap {
        return if existing == &receipt {
            Ok(ResponseFrame::BootstrapRestore(existing.clone()))
        } else {
            refused(operation, "restore_bootstrap_conflict")
        };
    }
    runtime.restore_bootstrap = Some(receipt.clone());
    Ok(ResponseFrame::BootstrapRestore(receipt))
}

pub(super) fn credential(
    runtime: &mut MemoryRuntime,
    frame: CredentialFrame,
    operation: WireOperation,
) -> crate::Result<ResponseFrame> {
    if runtime.install.is_none() && runtime.restore_bootstrap.is_none() {
        return refused(operation, "install_not_completed");
    }
    runtime.credential_delivered = true;
    Ok(ResponseFrame::Credential(CredentialReceipt {
        operation_id: frame.operation_id,
        installed: true,
    }))
}
