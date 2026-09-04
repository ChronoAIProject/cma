use std::path::{Path, PathBuf};
use std::sync::Arc;

use layer3_adapter::wire::{RequestFrame, ResponseFrame};
use layer3_adapter::{WorkspaceReceipt, WorkspaceRemoveReceipt};
use tokio::sync::Mutex;

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::workspace_journal::{self, Begin, Pending, Ticket};

pub(crate) async fn dispatch(
    config: RuntimeConfig,
    gate: Arc<Mutex<()>>,
    request: RequestFrame,
) -> Result<ResponseFrame> {
    tokio::spawn(async move {
        let _operation = gate.lock().await;
        recover_locked(&config).await?;
        dispatch_locked(&config, request).await
    })
    .await
    .map_err(join_error)?
}

/// Settle every orphaned Started record before another mutation is accepted.
pub(crate) async fn recover(config: RuntimeConfig, gate: Arc<Mutex<()>>) -> Result<()> {
    tokio::spawn(async move {
        let _operation = gate.lock().await;
        recover_locked(&config).await
    })
    .await
    .map_err(join_error)?
}

async fn dispatch_locked(config: &RuntimeConfig, request: RequestFrame) -> Result<ResponseFrame> {
    match request {
        RequestFrame::PrepareWorkspace(request) => prepare(config, request).await,
        RequestFrame::WorkspaceSetup(request) => setup(config, request).await,
        RequestFrame::RemoveWorkspace(request) => remove(config, request),
        _ => Err(RuntimeError::rejected(
            "workspace_operation_invalid",
            "workspace dispatcher received another operation",
        )),
    }
}

async fn recover_locked(config: &RuntimeConfig) -> Result<()> {
    for pending in workspace_journal::started(&config.state_root, &config.sandbox_id)? {
        match pending.kind.as_str() {
            "prepare" => recover_prepare(config, pending).await?,
            "setup" => {
                let request: layer3_adapter::WorkspaceSetupRequest = pending.request()?;
                if !request.has_valid_sha256()
                    || request.timeout_seconds == 0
                    || request.output_limit_bytes == 0
                {
                    return Err(invalid_digest());
                }
                pending.ticket.indeterminate(
                    "workspace setup may have executed before runtime restart; explicit recovery is required",
                )?;
            }
            "remove" => recover_remove(config, pending)?,
            _ => {
                return Err(RuntimeError::rejected(
                    "workspace_journal_corrupt",
                    "workspace journal contains an unknown operation kind",
                ))
            }
        }
    }
    Ok(())
}

async fn prepare(
    config: &RuntimeConfig,
    request: layer3_adapter::WorkspaceRequest,
) -> Result<ResponseFrame> {
    if !request.has_valid_sha256() {
        return Err(invalid_digest());
    }
    match workspace_journal::begin::<WorkspaceReceipt, _>(
        &config.state_root,
        &config.sandbox_id,
        &request.workspace_id,
        "prepare",
        &request.operation_id,
        &request.request_sha256,
        &request,
    )? {
        Begin::Completed(receipt) => Ok(ResponseFrame::PrepareWorkspace(receipt)),
        Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
        Begin::New(ticket) | Begin::Started(ticket) => {
            execute_prepare(config, request, ticket).await
        }
    }
}

async fn execute_prepare(
    config: &RuntimeConfig,
    request: layer3_adapter::WorkspaceRequest,
    ticket: Ticket,
) -> Result<ResponseFrame> {
    match recover_prepared_receipt(config, &request).await {
        Ok(Some(receipt)) => finish(ticket, Ok(receipt), ResponseFrame::PrepareWorkspace),
        Ok(None) => {
            let result = prepare_in_staging(config, &request).await;
            finish(ticket, result, ResponseFrame::PrepareWorkspace)
        }
        Err(error) => {
            let indeterminate = ticket.indeterminate(
                "workspace preparation left ambiguous staged or published state; remove it before retrying",
            )?;
            tracing::warn!(detail = %error, "workspace preparation recovery failed closed");
            Err(indeterminate)
        }
    }
}

async fn prepare_in_staging(
    config: &RuntimeConfig,
    request: &layer3_adapter::WorkspaceRequest,
) -> Result<WorkspaceReceipt> {
    let staging_root = staging_root(&config.home);
    std::fs::create_dir_all(&staging_root).map_err(|error| {
        RuntimeError::unavailable(
            "workspace_io_failed",
            format!("create workspace staging root: {error}"),
        )
    })?;
    let staged = staged_request(request);
    let receipt =
        crate::workspace_source::prepare(&staging_root, config.allow_local_git, &staged).await?;
    publish_staged(config, request, receipt.resolved_commit)
}

async fn recover_prepared_receipt(
    config: &RuntimeConfig,
    request: &layer3_adapter::WorkspaceRequest,
) -> Result<Option<WorkspaceReceipt>> {
    let published = config.home.join(&request.name);
    let staged_path = staging_root(&config.home).join(&request.workspace_id);
    if published.exists() && staged_path.exists() {
        return Err(RuntimeError::rejected(
            "workspace_prepare_indeterminate",
            "workspace has both staged and published directories",
        ));
    }
    if published.exists() {
        return crate::workspace_source::inspect_prepared(
            &config.home,
            config.allow_local_git,
            request,
        )
        .await;
    }
    if !staged_path.exists() {
        return Ok(None);
    }
    let staged = staged_request(request);
    let Some(receipt) = crate::workspace_source::inspect_prepared(
        &staging_root(&config.home),
        config.allow_local_git,
        &staged,
    )
    .await?
    else {
        return Ok(None);
    };
    publish_staged(config, request, receipt.resolved_commit).map(Some)
}

fn publish_staged(
    config: &RuntimeConfig,
    request: &layer3_adapter::WorkspaceRequest,
    resolved_commit: String,
) -> Result<WorkspaceReceipt> {
    let staged = staging_root(&config.home).join(&request.workspace_id);
    let published = config.home.join(&request.name);
    if published.exists() {
        return Err(RuntimeError::rejected(
            "workspace_exists",
            "workspace path already belongs to a live workspace",
        ));
    }
    std::fs::rename(&staged, &published).map_err(|error| {
        RuntimeError::unavailable(
            "workspace_io_failed",
            format!("publish prepared workspace: {error}"),
        )
    })?;
    Ok(WorkspaceReceipt {
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
        workspace_id: request.workspace_id.clone(),
        name: request.name.clone(),
        path: published.to_string_lossy().into_owned(),
        resolved_commit,
    })
}

async fn setup(
    config: &RuntimeConfig,
    request: layer3_adapter::WorkspaceSetupRequest,
) -> Result<ResponseFrame> {
    if !request.has_valid_sha256() {
        return Err(invalid_digest());
    }
    match workspace_journal::begin::<layer3_adapter::SetupReceipt, _>(
        &config.state_root,
        &config.sandbox_id,
        &request.workspace_id,
        "setup",
        &request.operation_id,
        &request.request_sha256,
        &request,
    )? {
        Begin::Completed(receipt) => Ok(ResponseFrame::WorkspaceSetup(receipt)),
        Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
        Begin::Started(ticket) => Err(ticket
            .indeterminate("workspace setup may have executed; explicit recovery is required")?),
        Begin::New(ticket) => {
            let result = crate::workspace_setup::run(config, &request).await;
            finish(ticket, result, ResponseFrame::WorkspaceSetup)
        }
    }
}

fn remove(
    config: &RuntimeConfig,
    request: layer3_adapter::WorkspaceRemoveRequest,
) -> Result<ResponseFrame> {
    if !request.has_valid_sha256() {
        return Err(invalid_digest());
    }
    match workspace_journal::begin::<WorkspaceRemoveReceipt, _>(
        &config.state_root,
        &config.sandbox_id,
        &request.workspace_id,
        "remove",
        &request.operation_id,
        &request.request_sha256,
        &request,
    )? {
        Begin::Completed(receipt) => Ok(ResponseFrame::RemoveWorkspace(receipt)),
        Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
        Begin::New(ticket) | Begin::Started(ticket) => execute_remove(config, request, ticket),
    }
}

fn execute_remove(
    config: &RuntimeConfig,
    request: layer3_adapter::WorkspaceRemoveRequest,
    ticket: Ticket,
) -> Result<ResponseFrame> {
    let result =
        crate::workspace_source::remove(&config.home, &request.workspace_id, &request.name);
    let receipt = WorkspaceRemoveReceipt {
        operation_id: request.operation_id,
        request_sha256: request.request_sha256,
        workspace_id: request.workspace_id,
    };
    match result {
        Ok(()) => {
            ticket.complete(&receipt)?;
            Ok(ResponseFrame::RemoveWorkspace(receipt))
        }
        Err(error) => {
            ticket.fail(&error)?;
            Err(error)
        }
    }
}

async fn recover_prepare(config: &RuntimeConfig, pending: Pending) -> Result<()> {
    let request: layer3_adapter::WorkspaceRequest = pending.request()?;
    if !request.has_valid_sha256() {
        return Err(invalid_digest());
    }
    execute_prepare(config, request, pending.ticket)
        .await
        .map(|_| ())
        .or_else(|error| {
            if error.code == "workspace_operation_indeterminate" {
                Ok(())
            } else {
                Err(error)
            }
        })
}

fn recover_remove(config: &RuntimeConfig, pending: Pending) -> Result<()> {
    let request: layer3_adapter::WorkspaceRemoveRequest = pending.request()?;
    if !request.has_valid_sha256() {
        return Err(invalid_digest());
    }
    execute_remove(config, request, pending.ticket).map(|_| ())
}

fn staged_request(request: &layer3_adapter::WorkspaceRequest) -> layer3_adapter::WorkspaceRequest {
    let mut staged = request.clone();
    staged.name.clone_from(&request.workspace_id);
    staged
}

fn staging_root(home: &Path) -> PathBuf {
    home.join(".cma").join("preparing")
}

fn finish<T: serde::Serialize>(
    ticket: Ticket,
    result: Result<T>,
    response: impl FnOnce(T) -> ResponseFrame,
) -> Result<ResponseFrame> {
    match result {
        Ok(receipt) => {
            ticket.complete(&receipt)?;
            Ok(response(receipt))
        }
        Err(error) => {
            ticket.fail(&error)?;
            Err(error)
        }
    }
}

fn invalid_digest() -> RuntimeError {
    RuntimeError::rejected(
        "workspace_request_digest_invalid",
        "workspace request digest does not match its execution fields",
    )
}

fn join_error(error: tokio::task::JoinError) -> RuntimeError {
    RuntimeError::unavailable(
        "workspace_operation_join_failed",
        format!("workspace operation task stopped: {error}"),
    )
}
