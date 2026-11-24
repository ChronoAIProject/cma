use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use layer3_adapter::{InstallReceipt, InstallRequest, RestoreBootstrapReceipt};
use serde::{Deserialize, Serialize};

use crate::command::{validate_commands, CommandLimits};
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::paths::path_is_directory;

pub(crate) const RECORD_VERSION: u8 = 2;
const STATE_DIRECTORY: &str = "runtime-state";
const STATE_FILE: &str = "install-v2.json";
const MAX_STATE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_STATE_ENTRIES: usize = 32;
const TEMP_PREFIX: &str = ".install-";
const TEMP_SUFFIX: &str = ".tmp";
static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum InstallRecord {
    Started {
        version: u8,
        sandbox_id: String,
        request: InstallRequest,
    },
    Completed {
        version: u8,
        sandbox_id: String,
        request: InstallRequest,
        receipt: InstallReceipt,
    },
    Failed {
        version: u8,
        sandbox_id: String,
        request: InstallRequest,
        code: String,
        detail: String,
        retryable: bool,
    },
    Indeterminate {
        version: u8,
        sandbox_id: String,
        request: InstallRequest,
    },
    Inherited {
        version: u8,
        sandbox_id: String,
        receipt: RestoreBootstrapReceipt,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecoveryState {
    Absent,
    Completed,
    Inherited(RestoreBootstrapReceipt),
    Terminal,
}

impl InstallRecord {
    pub(crate) fn started(config: &RuntimeConfig, request: &InstallRequest) -> Self {
        Self::Started {
            version: RECORD_VERSION,
            sandbox_id: config.sandbox_id.clone(),
            request: request.clone(),
        }
    }

    pub(crate) fn completed(
        config: &RuntimeConfig,
        request: &InstallRequest,
        receipt: InstallReceipt,
    ) -> Self {
        Self::Completed {
            version: RECORD_VERSION,
            sandbox_id: config.sandbox_id.clone(),
            request: request.clone(),
            receipt,
        }
    }

    pub(crate) fn failed(
        config: &RuntimeConfig,
        request: &InstallRequest,
        error: &RuntimeError,
    ) -> Self {
        Self::Failed {
            version: RECORD_VERSION,
            sandbox_id: config.sandbox_id.clone(),
            request: request.clone(),
            code: error.code.clone(),
            detail: error.detail.clone(),
            retryable: error.retryable,
        }
    }

    pub(crate) fn indeterminate(config: &RuntimeConfig, request: &InstallRequest) -> Self {
        Self::Indeterminate {
            version: RECORD_VERSION,
            sandbox_id: config.sandbox_id.clone(),
            request: request.clone(),
        }
    }

    pub(crate) fn inherited(config: &RuntimeConfig, receipt: RestoreBootstrapReceipt) -> Self {
        Self::Inherited {
            version: RECORD_VERSION,
            sandbox_id: config.sandbox_id.clone(),
            receipt,
        }
    }

    pub(crate) fn install_request(&self) -> Option<&InstallRequest> {
        match self {
            Self::Started { request, .. }
            | Self::Completed { request, .. }
            | Self::Failed { request, .. }
            | Self::Indeterminate { request, .. } => Some(request),
            Self::Inherited { .. } => None,
        }
    }

    pub(crate) fn bootstrap_receipt(&self) -> Option<&RestoreBootstrapReceipt> {
        match self {
            Self::Inherited { receipt, .. } => Some(receipt),
            _ => None,
        }
    }

    pub(crate) fn identity(&self) -> (u8, &str) {
        match self {
            Self::Started {
                version,
                sandbox_id,
                ..
            }
            | Self::Completed {
                version,
                sandbox_id,
                ..
            }
            | Self::Failed {
                version,
                sandbox_id,
                ..
            }
            | Self::Indeterminate {
                version,
                sandbox_id,
                ..
            }
            | Self::Inherited {
                version,
                sandbox_id,
                ..
            } => (*version, sandbox_id),
        }
    }

    fn terminal(&self) -> bool {
        !matches!(self, Self::Started { .. })
    }
}

pub(crate) fn validate_request(request: &InstallRequest) -> Result<()> {
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
    CommandLimits::install(
        request.timeout_seconds,
        request.total_timeout_seconds,
        request.output_limit_bytes,
    )?;
    validate_commands(&request.commands)
}

pub(crate) fn recover(config: &RuntimeConfig) -> Result<RecoveryState> {
    let Some(record) = load(config)? else {
        return Ok(RecoveryState::Absent);
    };
    match record {
        InstallRecord::Started { ref request, .. } => {
            replace(config, &InstallRecord::indeterminate(config, request))?;
            Ok(RecoveryState::Terminal)
        }
        InstallRecord::Completed { .. } => Ok(RecoveryState::Completed),
        InstallRecord::Inherited { receipt, .. } => Ok(RecoveryState::Inherited(receipt)),
        InstallRecord::Failed { .. } | InstallRecord::Indeterminate { .. } => {
            Ok(RecoveryState::Terminal)
        }
    }
}

pub(crate) fn load(config: &RuntimeConfig) -> Result<Option<InstallRecord>> {
    let Some(files) = scan(config)? else {
        return Ok(None);
    };
    for file in &files {
        crate::install_state_validation::validate_record(config, &file.record)?;
    }
    reconcile(config, files)
}

/// Inspect published installation authority without recovering journals or running work.
pub(crate) fn inspect(config: &RuntimeConfig) -> Result<Option<InstallRecord>> {
    let Some(files) = scan(config)? else {
        return Ok(None);
    };
    let mut canonical = None;
    for file in files {
        crate::install_state_validation::validate_record(config, &file.record)?;
        if file.kind == FileKind::Canonical {
            canonical = Some(file.record);
        } else {
            return Err(RuntimeError::unavailable(
                "install_state_recovery_pending",
                "installation state publication requires journal recovery",
            ));
        }
    }
    Ok(canonical)
}

pub(crate) fn write_initial(config: &RuntimeConfig, record: &InstallRecord) -> Result<bool> {
    crate::install_state_validation::validate_record(config, record)?;
    if !matches!(
        record,
        InstallRecord::Started { .. } | InstallRecord::Inherited { .. }
    ) {
        return Err(invalid("initial installation state is not admissible"));
    }
    let directory = ensure_state_directory(config)?;
    let path = directory.join(STATE_FILE);
    let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(error) => return Err(state_io("create installation intent", error)),
    };
    if let Err(error) = write_record(&mut file, record) {
        drop(file);
        let _ = crate::fs_durability::remove_file(&path);
        return Err(error);
    }
    crate::fs_durability::sync_directory(&directory)
        .map_err(|error| state_io("sync installation state directory", error))?;
    Ok(true)
}

pub(crate) fn replace(config: &RuntimeConfig, record: &InstallRecord) -> Result<()> {
    crate::install_state_validation::validate_record(config, record)?;
    if !record.terminal() {
        return Err(invalid("replacement installation state is not terminal"));
    }
    let directory = ensure_state_directory(config)?;
    let temporary = directory.join(format!(
        "{TEMP_PREFIX}{}-{}{TEMP_SUFFIX}",
        std::process::id(),
        NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| state_io("create installation state replacement", error))?;
    if let Err(error) = write_record(&mut file, record) {
        drop(file);
        let _ = crate::fs_durability::remove_file(&temporary);
        return Err(error);
    }
    drop(file);
    crate::fs_durability::rename(&temporary, &directory.join(STATE_FILE))
        .map_err(|error| state_io("publish installation state", error))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Canonical,
    Temporary,
}

struct StateFile {
    path: PathBuf,
    kind: FileKind,
    record: InstallRecord,
}

fn scan(config: &RuntimeConfig) -> Result<Option<Vec<StateFile>>> {
    let directory = state_directory(config);
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(state_io("inspect installation state directory", error)),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(invalid("installation state path is not a real directory")),
    }
    let mut files = Vec::new();
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| state_io("list installation state directory", error))?;
    for entry in entries {
        if files.len() == MAX_STATE_ENTRIES {
            return Err(invalid("installation state directory has too many entries"));
        }
        let entry = entry.map_err(|error| state_io("read installation state entry", error))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("installation state filename is not UTF-8"))?;
        if name == crate::ledger_format::BARRIER {
            crate::ledger_format::installation_marker(config)?;
            continue;
        }
        if name == crate::retirement_format::BARRIER {
            crate::retirement_format::installation_marker(config)?;
            continue;
        }
        let kind = classify(&name)?;
        files.push(StateFile {
            path: entry.path(),
            kind,
            record: read_record(&entry.path())?,
        });
    }
    Ok(Some(files))
}

fn reconcile(config: &RuntimeConfig, files: Vec<StateFile>) -> Result<Option<InstallRecord>> {
    let mut canonical = None;
    let mut temporary = Vec::new();
    for file in files {
        match file.kind {
            FileKind::Canonical if canonical.is_none() => canonical = Some(file),
            FileKind::Canonical => return Err(invalid("installation state is duplicated")),
            FileKind::Temporary => temporary.push(file),
        }
    }
    let Some(mut canonical) = canonical else {
        return if temporary.is_empty() {
            Ok(None)
        } else {
            Err(invalid(
                "installation state replacement has no canonical record",
            ))
        };
    };
    if temporary.is_empty() {
        return Ok(Some(canonical.record));
    }
    if temporary.iter().any(|candidate| {
        !candidate.record.terminal()
            || canonical.record.install_request().is_none()
            || candidate.record.install_request() != canonical.record.install_request()
    }) {
        return Err(invalid(
            "installation state replacement does not follow its canonical record",
        ));
    }
    let replacement = temporary.pop().expect("temporary state is non-empty");
    if temporary
        .iter()
        .any(|candidate| candidate.record != replacement.record)
    {
        return Err(invalid(
            "installation state has conflicting durable replacements",
        ));
    }
    if matches!(canonical.record, InstallRecord::Started { .. }) {
        crate::fs_durability::rename(&replacement.path, &state_path(config))
            .map_err(|error| state_io("recover installation state replacement", error))?;
        canonical.record = replacement.record;
    } else if canonical.record != replacement.record {
        return Err(invalid(
            "terminal installation state conflicts with a replacement",
        ));
    } else {
        remove_temporary(&replacement.path)?;
    }
    for stale in temporary {
        remove_temporary(&stale.path)?;
    }
    Ok(Some(canonical.record))
}

fn read_record(path: &Path) -> Result<InstallRecord> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options
        .open(path)
        .map_err(|error| state_io("open installation state", error))?;
    let metadata = file
        .metadata()
        .map_err(|error| state_io("inspect installation state handle", error))?;
    if !metadata.is_file() || metadata.len() > MAX_STATE_BYTES {
        return Err(invalid("installation state is not a bounded regular file"));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    (&mut file)
        .take(MAX_STATE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| state_io("read installation state", error))?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(invalid("installation state exceeds its size bound"));
    }
    serde_json::from_slice(&bytes).map_err(|error| invalid(error.to_string()))
}

fn write_record(file: &mut File, record: &InstallRecord) -> Result<()> {
    let bytes = serde_json::to_vec(record).map_err(|error| invalid(error.to_string()))?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        return Err(invalid("installation state exceeds its size bound"));
    }
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| state_io("write installation state", error))
}

fn ensure_state_directory(config: &RuntimeConfig) -> Result<PathBuf> {
    crate::fs_durability::create_dir_all(&config.checkpoint_root)
        .map_err(|error| state_io("create installation state root", error))?;
    let directory = state_directory(config);
    crate::fs_durability::create_dir_all(&directory)
        .map_err(|error| state_io("create installation state directory", error))?;
    if !path_is_directory(&directory) {
        return Err(invalid(
            "installation state directory is not a real directory",
        ));
    }
    Ok(directory)
}

fn classify(name: &str) -> Result<FileKind> {
    if name == STATE_FILE {
        return Ok(FileKind::Canonical);
    }
    if name.starts_with(TEMP_PREFIX)
        && name.ends_with(TEMP_SUFFIX)
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return Ok(FileKind::Temporary);
    }
    Err(invalid(
        "installation state directory contains an unknown file",
    ))
}

fn remove_temporary(path: &Path) -> Result<()> {
    crate::fs_durability::remove_file(path)
        .map_err(|error| state_io("remove recovered installation state", error))
}

fn state_directory(config: &RuntimeConfig) -> PathBuf {
    config.checkpoint_root.join(STATE_DIRECTORY)
}

fn state_path(config: &RuntimeConfig) -> PathBuf {
    state_directory(config).join(STATE_FILE)
}

fn invalid(detail: impl Into<String>) -> RuntimeError {
    RuntimeError::rejected("install_state_invalid", detail)
}

fn state_io(operation: &'static str, error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable("install_state_unavailable", format!("{operation}: {error}"))
}

pub(crate) fn inspection_temporary_name(name: &str) -> Result<bool> {
    classify(name).map(|kind| kind == FileKind::Temporary)
}
