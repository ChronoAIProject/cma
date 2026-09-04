use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use layer3_adapter::{InstallReceipt, InstallRequest};
use serde::{Deserialize, Serialize};

use crate::command::{run_shell_commands, validate_commands, CommandLimits};
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::paths::path_is_directory;

const RECORD_VERSION: u8 = 1;
const STATE_DIRECTORY: &str = "runtime-state";
const STATE_FILE: &str = "install-v1.json";
const MAX_STATE_BYTES: u64 = 512 * 1024;
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum InstallRecord {
    Started {
        version: u8,
        sandbox_id: String,
        operation_id: String,
        request_sha256: String,
    },
    Completed {
        version: u8,
        sandbox_id: String,
        operation_id: String,
        request_sha256: String,
        receipt: InstallReceipt,
    },
    Failed {
        version: u8,
        sandbox_id: String,
        operation_id: String,
        request_sha256: String,
        code: String,
        detail: String,
        retryable: bool,
    },
    Indeterminate {
        version: u8,
        sandbox_id: String,
        operation_id: String,
        request_sha256: String,
    },
}

impl InstallRecord {
    fn identity(&self) -> (u8, &str, &str, &str) {
        match self {
            Self::Started {
                version,
                sandbox_id,
                operation_id,
                request_sha256,
            }
            | Self::Completed {
                version,
                sandbox_id,
                operation_id,
                request_sha256,
                ..
            }
            | Self::Failed {
                version,
                sandbox_id,
                operation_id,
                request_sha256,
                ..
            }
            | Self::Indeterminate {
                version,
                sandbox_id,
                operation_id,
                request_sha256,
            } => (*version, sandbox_id, operation_id, request_sha256),
        }
    }
}

pub(crate) fn recover(config: &RuntimeConfig) -> Result<()> {
    let Some(record) = read_record(config)? else {
        return Ok(());
    };
    let (version, sandbox_id, _, _) = record.identity();
    validate_record_owner(config, version, sandbox_id)?;
    let InstallRecord::Started {
        version,
        sandbox_id,
        operation_id,
        request_sha256,
    } = record
    else {
        return Ok(());
    };
    replace_record(
        config,
        &InstallRecord::Indeterminate {
            version,
            sandbox_id,
            operation_id,
            request_sha256,
        },
    )
}

pub(crate) async fn run(
    config: &RuntimeConfig,
    request: &InstallRequest,
) -> Result<InstallReceipt> {
    validate_request(request)?;
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
    if let Some(record) = read_record(config)? {
        return replay(config, request, record);
    }
    let started = InstallRecord::Started {
        version: RECORD_VERSION,
        sandbox_id: config.sandbox_id.clone(),
        operation_id: request.operation_id.clone(),
        request_sha256: request.request_sha256.clone(),
    };
    if !write_initial_record(config, &started)? {
        return replay(
            config,
            request,
            read_record(config)?.ok_or_else(|| {
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
    let record = match &outcome {
        Ok(receipt) => InstallRecord::Completed {
            version: RECORD_VERSION,
            sandbox_id: config.sandbox_id.clone(),
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            receipt: receipt.clone(),
        },
        Err(error) => InstallRecord::Failed {
            version: RECORD_VERSION,
            sandbox_id: config.sandbox_id.clone(),
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            code: error.code.clone(),
            detail: error.detail.clone(),
            retryable: false,
        },
    };
    replace_record(config, &record)?;
    outcome
}

fn validate_request(request: &InstallRequest) -> Result<()> {
    if request.operation_id.trim().is_empty()
        || request.operation_id.len() > 256
        || request.operation_id.chars().any(char::is_control)
    {
        return Err(RuntimeError::rejected(
            "install_operation_invalid",
            "install operation id is empty, too large, or contains control characters",
        ));
    }
    if !request.has_valid_sha256() {
        return Err(RuntimeError::rejected(
            "install_digest_invalid",
            "install request digest does not match its execution fields",
        ));
    }
    CommandLimits::install(request.timeout_seconds, request.output_limit_bytes)?;
    validate_commands(&request.commands)
}

fn replay(
    config: &RuntimeConfig,
    request: &InstallRequest,
    record: InstallRecord,
) -> Result<InstallReceipt> {
    let (version, sandbox_id, operation_id, request_sha256) = record.identity();
    validate_record_owner(config, version, sandbox_id)?;
    if operation_id != request.operation_id || request_sha256 != request.request_sha256 {
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
        InstallRecord::Completed { receipt, .. }
            if receipt.operation_id == request.operation_id
                && receipt.request_sha256 == request.request_sha256 =>
        {
            Ok(receipt)
        }
        InstallRecord::Completed { .. } => Err(RuntimeError::rejected(
            "install_state_invalid",
            "completed installation state contains a mismatched receipt",
        )),
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

fn validate_record_owner(config: &RuntimeConfig, version: u8, sandbox_id: &str) -> Result<()> {
    if version != RECORD_VERSION || sandbox_id != config.sandbox_id {
        return Err(RuntimeError::rejected(
            "install_state_invalid",
            "installation state version or sandbox identity does not match this runtime",
        ));
    }
    Ok(())
}

fn state_directory(config: &RuntimeConfig) -> PathBuf {
    config.checkpoint_root.join(STATE_DIRECTORY)
}

fn state_path(config: &RuntimeConfig) -> PathBuf {
    state_directory(config).join(STATE_FILE)
}

fn ensure_state_directory(config: &RuntimeConfig) -> Result<PathBuf> {
    ensure_directory(&config.checkpoint_root)?;
    let directory = state_directory(config);
    ensure_directory(&directory)?;
    Ok(directory)
}

fn ensure_directory(path: &Path) -> Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(state_io("create installation state directory", error)),
    }
    if !path_is_directory(path) {
        return Err(RuntimeError::rejected(
            "install_state_invalid",
            "installation state directory is not a real directory",
        ));
    }
    Ok(())
}

fn read_record(config: &RuntimeConfig) -> Result<Option<InstallRecord>> {
    let path = state_path(config);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(state_io("inspect installation state", error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_STATE_BYTES
    {
        return Err(RuntimeError::rejected(
            "install_state_invalid",
            "installation state is not a bounded regular file",
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    File::open(&path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| state_io("read installation state", error))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| RuntimeError::rejected("install_state_invalid", error.to_string()))
}

fn write_initial_record(config: &RuntimeConfig, record: &InstallRecord) -> Result<bool> {
    let directory = ensure_state_directory(config)?;
    let path = directory.join(STATE_FILE);
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(state_io("create installation intent", error)),
    };
    write_record(&mut file, record)?;
    sync_directory(&directory)?;
    Ok(true)
}

fn replace_record(config: &RuntimeConfig, record: &InstallRecord) -> Result<()> {
    let directory = ensure_state_directory(config)?;
    let temporary = directory.join(format!(
        ".install-{}-{}.tmp",
        std::process::id(),
        NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| state_io("create installation state replacement", error))?;
    if let Err(error) = write_record(&mut file, record) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    drop(file);
    if let Err(error) = std::fs::rename(&temporary, directory.join(STATE_FILE)) {
        let _ = std::fs::remove_file(&temporary);
        return Err(state_io("publish installation state", error));
    }
    sync_directory(&directory)
}

fn write_record(file: &mut File, record: &InstallRecord) -> Result<()> {
    let bytes = serde_json::to_vec(record)
        .map_err(|error| RuntimeError::rejected("install_state_invalid", error.to_string()))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| state_io("write installation state", error))
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| state_io("sync installation state directory", error))
}

fn state_io(operation: &'static str, error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable("install_state_unavailable", format!("{operation}: {error}"))
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
