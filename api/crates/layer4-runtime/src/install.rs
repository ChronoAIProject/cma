use layer3_adapter::{InstallReceipt, InstallRequest};

use crate::command::{run_shell_commands, CommandLimits};
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::install_state::{self, InstallRecord, RecoveryState};

pub(crate) fn recover(config: &RuntimeConfig) -> Result<RecoveryState> {
    install_state::recover(config)
}

pub(crate) async fn run(
    config: &RuntimeConfig,
    request: &InstallRequest,
) -> Result<InstallReceipt> {
    install_state::validate_request(request)?;
    let config = config.clone();
    let request = request.clone();
    tokio::spawn(async move { run_owned(&config, &request).await })
        .await
        .map_err(|error| {
            RuntimeError::unavailable(
                "install_task_failed",
                format!("installation task did not return: {error}"),
            )
        })?
}

async fn run_owned(config: &RuntimeConfig, request: &InstallRequest) -> Result<InstallReceipt> {
    if let Some(record) = install_state::load(config)? {
        return replay(request, record);
    }
    let started = InstallRecord::started(config, request);
    if !install_state::write_initial(config, &started)? {
        return replay(
            request,
            install_state::load(config)?.ok_or_else(|| {
                RuntimeError::unavailable(
                    "install_state_unavailable",
                    "installation state disappeared during admission",
                )
            })?,
        );
    }

    let limits = CommandLimits::install(request.timeout_seconds, request.output_limit_bytes)?;
    let outcome = run_shell_commands(&request.commands, &config.home, &config.install_env, limits)
        .await
        .map(|commands| InstallReceipt {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            commands,
        });
    let terminal = match &outcome {
        Ok(receipt) => InstallRecord::completed(config, request, receipt.clone()),
        Err(error) if error.retryable => InstallRecord::indeterminate(config, request),
        Err(error) => InstallRecord::failed(config, request, error),
    };
    install_state::replace(config, &terminal)?;
    outcome
}

fn replay(request: &InstallRequest, record: InstallRecord) -> Result<InstallReceipt> {
    if record.request() != request {
        return Err(RuntimeError::rejected(
            "install_replay_mismatch",
            "completed or pending install does not match the replayed request",
        ));
    }
    match record {
        InstallRecord::Started { .. } => Err(RuntimeError::unavailable(
            "install_in_progress",
            "the admitted installation has not recorded an outcome yet",
        )),
        InstallRecord::Completed { receipt, .. } => Ok(receipt),
        InstallRecord::Failed {
            code,
            detail,
            retryable,
            ..
        } => Err(RuntimeError::recorded(code, detail, retryable)),
        InstallRecord::Indeterminate { .. } => Err(RuntimeError::rejected(
            "install_outcome_unknown",
            "a prior installation stopped without recording its outcome",
        )),
    }
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
