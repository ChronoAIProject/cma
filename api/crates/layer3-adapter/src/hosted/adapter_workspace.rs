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
    if !(1..=1_000).contains(&request.clone_depth) {
        return Err(AdapterError::Rejected(
            "workspace clone depth is invalid".into(),
        ));
    }
    let state = adapter
        .known_state(handle)
        .await
        .inspect_err(|_| crate::workspace_prepare_observation::inspection())?;
    validate_operation(
        &request.operation_id,
        &request.request_sha256,
        request.has_valid_sha256(),
    )?;
    if request.repository_connection_id.is_some()
        && !adapter
            .inspect_runtime_status(handle)
            .await
            .inspect_err(|_| crate::workspace_prepare_observation::inspection())?
            .github_repository_scope
    {
        return Err(repository_scope_unsupported());
    }
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
        ResponseFrame::PrepareWorkspace(_) => {
            crate::workspace_prepare_observation::invalid_receipt();
            Err(AdapterError::Rejected(
                "workspace receipt names a different workspace".into(),
            ))
        }
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
    if request.timeout_seconds == 0
        || request.timeout_seconds > 600
        || request.total_timeout_seconds < request.timeout_seconds
        || request.total_timeout_seconds > 1_800
        || request.output_limit_bytes == 0
    {
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

pub(super) async fn removal_support(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
) -> crate::Result<crate::WorkspaceRemovalSupport> {
    let status = adapter.inspect_runtime_status(handle).await?;
    let identity = crate::RuntimeStatusRequest {
        sandbox_id: status.sandbox_id.clone(),
        generation: status.generation,
        home_identity: status.home_identity.clone(),
    };
    let ResponseFrame::WorkspaceRemovalSupport(support) = adapter
        .exchange(
            handle,
            RequestFrame::WorkspaceRemovalSupport(identity.clone()),
        )
        .await?
    else {
        return Err(AdapterError::unexpected_reply("Workspace removal support"));
    };
    support.validate_for(&identity)?;
    if !crate::same_runtime_writer(&status, &support.runtime) {
        return Err(AdapterError::Rejected(
            "Workspace removal probe changed runtime writer".into(),
        ));
    }
    adapter.verify_bound_home(handle).await?;
    Ok(support)
}

pub(super) async fn reconcile_removal(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    expected_runtime: &crate::RuntimeStatus,
    request: &WorkspaceRemoveRequest,
) -> crate::Result<crate::WorkspaceRemovalObservation> {
    let frame = crate::WorkspaceRemovalRequest {
        runtime: expected_runtime.clone(),
        request: request.clone(),
    };
    frame.validate()?;
    let current = adapter.inspect_runtime_status(handle).await?;
    if !crate::same_runtime_writer(&current, expected_runtime) {
        return Err(AdapterError::Rejected(
            "Workspace removal runtime writer changed".into(),
        ));
    }
    let ResponseFrame::ReconcileWorkspaceRemoval(observation) = adapter
        .exchange(
            handle,
            RequestFrame::ReconcileWorkspaceRemoval(frame.clone()),
        )
        .await?
    else {
        return Err(AdapterError::unexpected_reply(
            "Workspace removal observation",
        ));
    };
    observation.validate_for(&frame)?;
    adapter.verify_bound_home(handle).await?;
    Ok(observation)
}

pub(super) async fn recover_prepare(
    adapter: &HostedAdapter,
    handle: &RuntimeHandle,
    request: &crate::WorkspacePrepareRecoveryRequest,
) -> crate::Result<crate::WorkspacePrepareRecoveryReceipt> {
    request.validate()?;
    let state = adapter
        .known_state(handle)
        .await
        .inspect_err(|_| crate::workspace_prepare_observation::inspection())?;
    let runtime = adapter
        .inspect_runtime_status(handle)
        .await
        .inspect_err(|_| crate::workspace_prepare_observation::inspection())?;
    if request.original.repository_connection_id.is_some() && !runtime.github_repository_scope {
        return Err(repository_scope_unsupported());
    }
    let frame = crate::WorkspacePrepareRecoveryFrame {
        runtime,
        request: request.clone(),
    };
    frame.validate()?;
    let ResponseFrame::RecoverWorkspacePrepare(receipt) = adapter
        .exchange(handle, RequestFrame::RecoverWorkspacePrepare(frame))
        .await?
    else {
        return Err(AdapterError::unexpected_reply(
            "Workspace preparation recovery",
        ));
    };
    if Path::new(&receipt.prepared.path)
        != Path::new(&state.home_directory).join(&request.original.name)
    {
        crate::workspace_prepare_observation::invalid_receipt();
        return Err(AdapterError::Rejected(
            "recovered Workspace receipt changed its original path".into(),
        ));
    }
    adapter.verify_bound_home(handle).await?;
    Ok(receipt)
}

fn repository_scope_unsupported() -> AdapterError {
    AdapterError::Unsupported("runtime does not support verified GitHub repository access".into())
}
