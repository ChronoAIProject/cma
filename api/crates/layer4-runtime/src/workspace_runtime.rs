use std::path::{Path, PathBuf};
use std::sync::Arc;

use layer3_adapter::wire::{RequestFrame, ResponseFrame};
use layer3_adapter::{WorkspaceReceipt, WorkspaceRemoveReceipt};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::workspace_journal::{self, Begin, Pending, Ticket};

pub(crate) async fn dispatch(
    config: RuntimeConfig,
    operation_guard: OwnedMutexGuard<()>,
    held_checkpoint: Arc<Mutex<Option<crate::supervisor::HeldCheckpoint>>>,
    removal_writer: Option<std::fs::File>,
    request: RequestFrame,
) -> Result<ResponseFrame> {
    tokio::spawn(async move {
        // The supervisor checked live and durable Agent inventory while
        // holding this same gate; retain it even if the caller disconnects.
        let _operation = operation_guard;
        let _removal_writer = removal_writer;
        crate::persistent_home::verify(&config)?;
        if held_checkpoint.lock().await.is_some() {
            return Err(RuntimeError::rejected(
                "checkpoint_writers_held",
                "operation is not permitted while checkpoint writers are held",
            ));
        }
        if !matches!(
            crate::install::recover(&config)?,
            crate::install_state::RecoveryState::Completed
                | crate::install_state::RecoveryState::Inherited(_)
        ) {
            return Err(RuntimeError::rejected(
                "install_state_invalid",
                "workspace mutation requires a durable completed installation",
            ));
        }
        dispatch_locked(&config, request).await
    })
    .await
    .map_err(join_error)?
}

async fn dispatch_locked(config: &RuntimeConfig, request: RequestFrame) -> Result<ResponseFrame> {
    match request {
        RequestFrame::PrepareWorkspace(request) => prepare(config, request).await,
        RequestFrame::RecoverWorkspacePrepare(frame) => {
            crate::workspace_prepare_recovery::execute(config, &frame.request)
                .await
                .map(ResponseFrame::RecoverWorkspacePrepare)
        }
        RequestFrame::WorkspaceSetup(request) => setup(config, request).await,
        RequestFrame::RemoveWorkspace(request) => remove(config, request),
        _ => Err(RuntimeError::rejected(
            "workspace_operation_invalid",
            "workspace dispatcher received another operation",
        )),
    }
}

pub(crate) async fn recover_locked(
    config: &RuntimeConfig,
    supervisor: &crate::supervisor::Supervisor,
) -> Result<()> {
    for pending in workspace_journal::started(
        &config.state_root,
        &config.sandbox_id,
        &config.home,
        config.allow_local_git,
    )? {
        match pending.kind.as_str() {
            "prepare" => {
                let _writer = supervisor.removal_writer_guard()?;
                recover_prepare(config, pending).await?;
            }
            "setup" => {
                let request: layer3_adapter::WorkspaceSetupRequest = pending.request()?;
                if !request.has_valid_sha256()
                    || crate::command::CommandLimits::install(
                        request.timeout_seconds,
                        request.total_timeout_seconds,
                        request.output_limit_bytes,
                    )
                    .is_err()
                {
                    return Err(invalid_digest());
                }
                pending.ticket.indeterminate(
                    "workspace setup may have executed before runtime restart; explicit recovery is required",
                )?;
            }
            "remove" => recover_remove(config, supervisor, pending).await?,
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
    crate::workspace_source::validate_clone_depth(request.clone_depth)?;
    if !request.has_valid_sha256() {
        return Err(invalid_digest());
    }
    match workspace_journal::begin::<WorkspaceReceipt, _>(
        &config.state_root,
        &config.home,
        config.allow_local_git,
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
        Ok(Some(receipt)) => {
            finish_recoverable(ticket, Ok(receipt), ResponseFrame::PrepareWorkspace)
        }
        Ok(None) => {
            let result = prepare_in_staging(config, &request).await;
            finish_recoverable(ticket, result, ResponseFrame::PrepareWorkspace)
        }
        Err(error) if error.retryable => Err(error),
        Err(error) if error.code == "workspace_prepare_indeterminate" => {
            let indeterminate = ticket.indeterminate(
                "workspace preparation left ambiguous staged or published state; explicit Workspace recovery is required",
            )?;
            tracing::warn!(detail = %error, "workspace preparation recovery failed closed");
            Err(indeterminate)
        }
        Err(error) => finish_recoverable(ticket, Err(error), ResponseFrame::PrepareWorkspace),
    }
}

async fn prepare_in_staging(
    config: &RuntimeConfig,
    request: &layer3_adapter::WorkspaceRequest,
) -> Result<WorkspaceReceipt> {
    let staging_root = staging_root(&config.home);
    crate::fs_durability::create_dir_all(&staging_root).map_err(|error| {
        RuntimeError::unavailable(
            "workspace_io_failed",
            format!("create workspace staging root: {error}"),
        )
    })?;
    let staged = staged_request(request);
    let receipt = if matches!(
        request.source,
        layer3_adapter::WorkspaceSource::RepositorySnapshot { .. }
            | layer3_adapter::WorkspaceSource::PublicRepositorySnapshot { .. }
    ) {
        crate::repository_snapshot::prepare(config, &staging_root, &staged).await?
    } else {
        crate::workspace_source::prepare(&staging_root, config.allow_local_git, &staged).await?
    };
    publish_staged(
        config,
        request,
        receipt.resolved_commit,
        receipt.source_commit,
    )
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
    // Only the exact journal-owned staging path can be an unfinished clone.
    // The identity is written after Git succeeds; its absence cannot be healed
    // by replaying inspection. Preserve the files and settle the preparation.
    match std::fs::symlink_metadata(staged_path.join(".git/info/cma-workspace-id")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(RuntimeError::rejected(
                "workspace_prepare_indeterminate",
                "staged workspace preparation has no completed identity metadata",
            ));
        }
        Err(error) => {
            return Err(RuntimeError::unavailable(
                "workspace_io_failed",
                format!("inspect staged workspace identity: {error}"),
            ));
        }
        Ok(_) => {}
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
    publish_staged(
        config,
        request,
        receipt.resolved_commit,
        receipt.source_commit,
    )
    .map(Some)
}

fn publish_staged(
    config: &RuntimeConfig,
    request: &layer3_adapter::WorkspaceRequest,
    resolved_commit: String,
    source_commit: Option<String>,
) -> Result<WorkspaceReceipt> {
    let staged = staging_root(&config.home).join(&request.workspace_id);
    let published = config.home.join(&request.name);
    if published.exists() {
        return Err(RuntimeError::rejected(
            "workspace_exists",
            "workspace path already belongs to a live workspace",
        ));
    }
    crate::fs_durability::rename(&staged, &published).map_err(|error| {
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
        source_commit,
    })
}

async fn setup(
    config: &RuntimeConfig,
    request: layer3_adapter::WorkspaceSetupRequest,
) -> Result<ResponseFrame> {
    crate::command::CommandLimits::install(
        request.timeout_seconds,
        request.total_timeout_seconds,
        request.output_limit_bytes,
    )?;
    if !request.has_valid_sha256() {
        return Err(invalid_digest());
    }
    match workspace_journal::begin::<layer3_adapter::SetupReceipt, _>(
        &config.state_root,
        &config.home,
        config.allow_local_git,
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
            finish_setup(ticket, result)
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
        &config.home,
        config.allow_local_git,
        &config.sandbox_id,
        &request.workspace_id,
        "remove",
        &request.operation_id,
        &request.request_sha256,
        &request,
    )? {
        Begin::Completed(receipt) => {
            match crate::workspace_removal_paths::inspect(&config.home, &request.workspace_id, &request.name)? {
                crate::workspace_removal_paths::Presence::Original => Err(RuntimeError::rejected(
                    "workspace_removal_reconciliation_required",
                    "the removed Workspace reappeared; a current boot-correlated reconciliation is required",
                )),
                crate::workspace_removal_paths::Presence::Absent | crate::workspace_removal_paths::Presence::Successor => Ok(ResponseFrame::RemoveWorkspace(receipt)),
            }
        }
        Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
        Begin::New(ticket) | Begin::Started(ticket) => {
            if crate::workspace_removal_intent::requires_current(config, &request)? {
                return Err(RuntimeError::rejected(
                    "workspace_removal_reconciliation_required",
                    "this removal requires a current boot-correlated reconciliation",
                ));
            }
            execute_remove(config, request, ticket)
        }
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
            if !error.retryable {
                ticket.fail(&error)?;
            }
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
        .or_else(|error| (!error.retryable).then_some(()).ok_or(error))
}

async fn recover_remove(
    config: &RuntimeConfig,
    supervisor: &crate::supervisor::Supervisor,
    pending: Pending,
) -> Result<()> {
    let request: layer3_adapter::WorkspaceRemoveRequest = pending.request()?;
    if !request.has_valid_sha256() {
        return Err(invalid_digest());
    }
    if crate::workspace_removal_intent::requires_current(config, &request)? {
        return Ok(());
    }
    let _removal_writer = supervisor.removal_writer_guard()?;
    match supervisor.ensure_workspace_idle(&request.name).await {
        // Keep the original intent pending while allowing the Agent's close
        // request to run. Recovery will retry after its durable close receipt.
        Err(error) if error.code == "workspace_agent_open" => return Ok(()),
        result => result?,
    }
    execute_remove(config, request, pending.ticket)
        .map(|_| ())
        .or_else(|error| (!error.retryable).then_some(()).ok_or(error))
}

fn staged_request(request: &layer3_adapter::WorkspaceRequest) -> layer3_adapter::WorkspaceRequest {
    let mut staged = request.clone();
    staged.name.clone_from(&request.workspace_id);
    staged
}

fn staging_root(home: &Path) -> PathBuf {
    home.join(".cma").join("preparing")
}

fn finish_recoverable<T: serde::Serialize>(
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
            if !error.retryable {
                ticket.fail(&error)?;
            }
            Err(error)
        }
    }
}

fn finish_setup(
    ticket: Ticket,
    result: Result<layer3_adapter::SetupReceipt>,
) -> Result<ResponseFrame> {
    match result {
        Ok(receipt) => {
            ticket.complete(&receipt)?;
            Ok(ResponseFrame::WorkspaceSetup(receipt))
        }
        Err(error) if error.retryable => Err(ticket.indeterminate(
            "workspace setup may have executed before its outcome became durable",
        )?),
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
