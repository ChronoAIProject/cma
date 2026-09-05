use std::path::Path;

use super::super::reply::{valid_git_commit, valid_sha256, validate_command_receipts};
use super::HostedAdapter;
use crate::wire::{RequestFrame, ResponseFrame};
use crate::{
    AdapterError, RuntimeHandle, SetupReceipt, WorkspaceReceipt, WorkspaceRemoveRequest,
    WorkspaceRequest, WorkspaceSetupRequest,
};

pub(super) async fn prepare(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    request: &WorkspaceRequest,
) -> crate::Result<WorkspaceReceipt> {
    let state = adapter.known_state(handle).await?;
    validate_operation(
        &request.operation_id,
        &request.request_sha256,
        request.has_valid_sha256(),
    )?;
    match adapter
        .exchange(handle, RequestFrame::PrepareWorkspace(request.clone()))
        .await?
    {
        ResponseFrame::PrepareWorkspace(receipt)
            if receipt.name == request.name
                && receipt.workspace_id == request.workspace_id
                && receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && Path::new(&receipt.path)
                    == Path::new(&state.home_directory).join(&request.name)
                && valid_git_commit(&receipt.resolved_commit) =>
        {
            Ok(receipt)
        }
        ResponseFrame::PrepareWorkspace(_) => Err(AdapterError::Rejected(
            "workspace receipt names a different workspace".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

pub(super) async fn setup(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    request: &WorkspaceSetupRequest,
) -> crate::Result<SetupReceipt> {
    adapter.known_state(handle).await?;
    validate_operation(
        &request.operation_id,
        &request.request_sha256,
        request.has_valid_sha256(),
    )?;
    if request.timeout_seconds == 0 || request.output_limit_bytes == 0 {
        return Err(AdapterError::Rejected(
            "workspace setup command bounds are invalid".into(),
        ));
    }
    match adapter
        .exchange(handle, RequestFrame::WorkspaceSetup(request.clone()))
        .await?
    {
        ResponseFrame::WorkspaceSetup(receipt) => {
            if receipt.operation_id != request.operation_id
                || receipt.request_sha256 != request.request_sha256
                || receipt.workspace_id != request.workspace_id
            {
                return Err(AdapterError::Rejected(
                    "runtime returned a receipt for another workspace setup operation".into(),
                ));
            }
            validate_command_receipts(&receipt.commands, request.commands.len())?;
            Ok(receipt)
        }
        _ => unreachable!("exchange verifies response operation"),
    }
}

pub(super) async fn remove(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    request: &WorkspaceRemoveRequest,
) -> crate::Result<()> {
    adapter.known_state(handle).await?;
    validate_operation(
        &request.operation_id,
        &request.request_sha256,
        request.has_valid_sha256(),
    )?;
    match adapter
        .exchange(handle, RequestFrame::RemoveWorkspace(request.clone()))
        .await?
    {
        ResponseFrame::RemoveWorkspace(receipt)
            if receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256
                && receipt.workspace_id == request.workspace_id =>
        {
            Ok(())
        }
        ResponseFrame::RemoveWorkspace(_) => Err(AdapterError::Rejected(
            "runtime returned a receipt for another workspace removal operation".into(),
        )),
        _ => unreachable!("exchange verifies response operation"),
    }
}

fn validate_operation(
    operation_id: &str,
    request_sha256: &str,
    digest_matches: bool,
) -> crate::Result<()> {
    if operation_id.trim().is_empty()
        || operation_id.len() > 256
        || operation_id.chars().any(char::is_control)
        || !valid_sha256(request_sha256)
        || !digest_matches
    {
        return Err(AdapterError::Rejected(
            "workspace operation id or request digest is invalid".into(),
        ));
    }
    Ok(())
}
