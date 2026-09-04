use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::failure::{Result, RuntimeError};

const JOURNAL_VERSION: u32 = 1;
const MAX_JOURNAL_BYTES: u64 = 8 * 1024 * 1024;
const MAX_JOURNAL_ENTRIES: usize = 1_024;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Started,
    Completed,
    Failed,
    Indeterminate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredFailure {
    code: String,
    detail: String,
    retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    sandbox_id: String,
    workspace_id: String,
    operation_id: String,
    request_sha256: String,
    kind: String,
    status: Status,
    request: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    receipt: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<StoredFailure>,
}

pub(crate) enum Begin<T> {
    New(Ticket),
    Started(Ticket),
    Completed(T),
    Failed(RuntimeError),
    Indeterminate(RuntimeError),
}

#[derive(Debug, Clone)]
pub(crate) struct Ticket {
    path: PathBuf,
    record: Record,
}

pub(crate) struct Pending {
    pub kind: String,
    pub ticket: Ticket,
}

pub(crate) fn begin<T: DeserializeOwned, R: Serialize>(
    state_root: &Path,
    sandbox_id: &str,
    workspace_id: &str,
    kind: &str,
    operation_id: &str,
    request_sha256: &str,
    request: &R,
) -> Result<Begin<T>> {
    validate_identity(workspace_id, kind, operation_id, request_sha256)?;
    let request = serde_json::to_value(request).map_err(|error| {
        RuntimeError::rejected(
            "workspace_request_invalid",
            format!("encode workspace operation request: {error}"),
        )
    })?;
    let path = journal_path(state_root, operation_id);
    match read_record(&path)? {
        Some(record) => resolve_existing(
            path,
            record,
            sandbox_id,
            workspace_id,
            kind,
            operation_id,
            request_sha256,
            &request,
        ),
        None => create_started(
            state_root,
            sandbox_id,
            workspace_id,
            kind,
            operation_id,
            request_sha256,
            request,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn create_started<T: DeserializeOwned>(
    state_root: &Path,
    sandbox_id: &str,
    workspace_id: &str,
    kind: &str,
    operation_id: &str,
    request_sha256: &str,
    request: serde_json::Value,
) -> Result<Begin<T>> {
    let record = Record {
        version: JOURNAL_VERSION,
        sandbox_id: sandbox_id.into(),
        workspace_id: workspace_id.into(),
        operation_id: operation_id.into(),
        request_sha256: request_sha256.into(),
        kind: kind.into(),
        status: Status::Started,
        request,
        receipt: None,
        failure: None,
    };
    let ticket = Ticket {
        path: journal_path(state_root, operation_id),
        record,
    };
    match ticket.persist_new() {
        Ok(()) => Ok(Begin::New(ticket)),
        Err(error) if error.code == "workspace_journal_raced" => {
            let record = read_record(&ticket.path)?
                .ok_or_else(|| corrupt("raced workspace journal disappeared"))?;
            resolve_existing(
                ticket.path,
                record,
                sandbox_id,
                workspace_id,
                kind,
                operation_id,
                request_sha256,
                &ticket.record.request,
            )
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn started(state_root: &Path, sandbox_id: &str) -> Result<Vec<Pending>> {
    let directory = journal_directory(state_root);
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error("inspect workspace operation journal", error)),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(corrupt(
                "workspace operation journal is not a real directory",
            ))
        }
    }
    let mut paths = std::fs::read_dir(&directory)
        .map_err(|error| io_error("list workspace operation journal", error))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| io_error("read workspace journal entry", error))
        })
        .collect::<Result<Vec<_>>>()?;
    paths.sort();
    if paths.len() > MAX_JOURNAL_ENTRIES {
        return Err(corrupt(
            "workspace operation journal contains too many entries",
        ));
    }
    let mut pending = Vec::new();
    for path in paths {
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            return Err(corrupt(
                "workspace operation journal contains an unknown file",
            ));
        }
        let record = read_record(&path)?
            .ok_or_else(|| corrupt("workspace operation journal entry disappeared"))?;
        validate_stored(&record)?;
        if record.sandbox_id != sandbox_id {
            return Err(corrupt(
                "workspace operation journal belongs to another sandbox",
            ));
        }
        if matches!(record.status, Status::Started) {
            pending.push(Pending {
                kind: record.kind.clone(),
                ticket: Ticket { path, record },
            });
        }
    }
    Ok(pending)
}

impl Pending {
    pub(crate) fn request<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(self.ticket.record.request.clone()).map_err(|error| {
            corrupt(&format!(
                "decode pending workspace operation request: {error}"
            ))
        })
    }
}

impl Ticket {
    pub(crate) fn complete<T: Serialize>(mut self, receipt: &T) -> Result<()> {
        self.record.status = Status::Completed;
        self.record.receipt = Some(serde_json::to_value(receipt).map_err(|error| {
            RuntimeError::unavailable(
                "workspace_journal_io_failed",
                format!("encode workspace operation receipt: {error}"),
            )
        })?);
        self.record.failure = None;
        self.persist_atomic()
    }

    pub(crate) fn fail(mut self, error: &RuntimeError) -> Result<()> {
        self.record.status = Status::Failed;
        self.record.receipt = None;
        self.record.failure = Some(failure(error));
        self.persist_atomic()
    }

    pub(crate) fn indeterminate(mut self, detail: &str) -> Result<RuntimeError> {
        let error = RuntimeError::rejected("workspace_operation_indeterminate", detail);
        self.record.status = Status::Indeterminate;
        self.record.receipt = None;
        self.record.failure = Some(failure(&error));
        self.persist_atomic()?;
        Ok(error)
    }

    fn persist_new(&self) -> Result<()> {
        let parent = prepare_parent(&self.path)?;
        let bytes = encode_record(&self.record)?;
        let mut file = match OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(RuntimeError::rejected(
                    "workspace_journal_raced",
                    "workspace operation journal was created concurrently",
                ))
            }
            Err(error) => return Err(io_error("create workspace operation journal", error)),
        };
        durable_write(&mut file, &bytes)?;
        sync_directory(&parent)
    }

    fn persist_atomic(&self) -> Result<()> {
        let parent = prepare_parent(&self.path)?;
        let bytes = encode_record(&self.record)?;
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".workspace-journal-{}-{sequence}.tmp",
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| io_error("create workspace journal temporary file", error))?;
        if let Err(error) = durable_write(&mut file, &bytes) {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
        drop(file);
        if let Err(error) = std::fs::rename(&temporary, &self.path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(io_error("commit workspace operation journal", error));
        }
        sync_directory(&parent)
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_existing<T: DeserializeOwned>(
    path: PathBuf,
    record: Record,
    sandbox_id: &str,
    workspace_id: &str,
    kind: &str,
    operation_id: &str,
    request_sha256: &str,
    request: &serde_json::Value,
) -> Result<Begin<T>> {
    validate_stored(&record)?;
    if record.sandbox_id != sandbox_id
        || record.workspace_id != workspace_id
        || record.operation_id != operation_id
        || record.request_sha256 != request_sha256
        || record.kind != kind
        || &record.request != request
    {
        return Err(RuntimeError::rejected(
            "workspace_replay_mismatch",
            "workspace operation replay differs from the admitted request",
        ));
    }
    let ticket = Ticket { path, record };
    match ticket.record.status {
        Status::Started => Ok(Begin::Started(ticket)),
        Status::Completed => {
            let value = ticket
                .record
                .receipt
                .ok_or_else(|| corrupt("completed workspace operation has no receipt"))?;
            serde_json::from_value(value)
                .map(Begin::Completed)
                .map_err(|error| corrupt(&format!("decode workspace operation receipt: {error}")))
        }
        Status::Failed => Ok(Begin::Failed(stored_error(&ticket.record)?)),
        Status::Indeterminate => Ok(Begin::Indeterminate(stored_error(&ticket.record)?)),
    }
}

fn validate_identity(
    workspace_id: &str,
    kind: &str,
    operation_id: &str,
    request_sha256: &str,
) -> Result<()> {
    let expected = format!("workspace:{workspace_id}:{kind}:v1");
    if operation_id != expected
        || workspace_id.is_empty()
        || workspace_id.len() > 128
        || !workspace_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || request_sha256.len() != 64
        || !request_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(RuntimeError::rejected(
            "workspace_operation_invalid",
            "workspace operation identity is invalid",
        ));
    }
    Ok(())
}

fn validate_stored(record: &Record) -> Result<()> {
    if record.version != JOURNAL_VERSION {
        return Err(corrupt(
            "workspace operation journal version is unsupported",
        ));
    }
    validate_identity(
        &record.workspace_id,
        &record.kind,
        &record.operation_id,
        &record.request_sha256,
    )?;
    let terminal_shape = match record.status {
        Status::Started => record.receipt.is_none() && record.failure.is_none(),
        Status::Completed => record.receipt.is_some() && record.failure.is_none(),
        Status::Failed | Status::Indeterminate => {
            record.receipt.is_none() && record.failure.is_some()
        }
    };
    if !terminal_shape {
        return Err(corrupt("workspace journal terminal shape is invalid"));
    }
    validate_request_record(record)
}

fn validate_request_record(record: &Record) -> Result<()> {
    let valid = match record.kind.as_str() {
        "prepare" => {
            serde_json::from_value::<layer3_adapter::WorkspaceRequest>(record.request.clone())
                .is_ok_and(|request| {
                    request.workspace_id == record.workspace_id
                        && request.operation_id == record.operation_id
                        && request.request_sha256 == record.request_sha256
                        && request.has_valid_sha256()
                })
        }
        "setup" => {
            serde_json::from_value::<layer3_adapter::WorkspaceSetupRequest>(record.request.clone())
                .is_ok_and(|request| {
                    request.workspace_id == record.workspace_id
                        && request.operation_id == record.operation_id
                        && request.request_sha256 == record.request_sha256
                        && request.has_valid_sha256()
                        && request.timeout_seconds > 0
                        && request.output_limit_bytes > 0
                })
        }
        "remove" => {
            serde_json::from_value::<layer3_adapter::WorkspaceRemoveRequest>(record.request.clone())
                .is_ok_and(|request| {
                    request.workspace_id == record.workspace_id
                        && request.operation_id == record.operation_id
                        && request.request_sha256 == record.request_sha256
                        && request.has_valid_sha256()
                })
        }
        _ => false,
    };
    valid
        .then_some(())
        .ok_or_else(|| corrupt("workspace journal request or digest is invalid"))
}

fn read_record(path: &Path) -> Result<Option<Record>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("inspect workspace journal entry", error)),
    };
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_JOURNAL_BYTES
    {
        return Err(corrupt("workspace journal entry is unsafe or oversized"));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| io_error("read workspace operation journal entry", error))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| corrupt(&format!("decode workspace operation journal: {error}")))
}

fn encode_record(record: &Record) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(record).map_err(|error| {
        RuntimeError::unavailable(
            "workspace_journal_io_failed",
            format!("encode workspace operation journal: {error}"),
        )
    })?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(corrupt("workspace operation journal entry is oversized"));
    }
    Ok(bytes)
}

fn prepare_parent(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("workspace operation journal path has no parent"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| io_error("create workspace operation journal", error))?;
    let metadata = std::fs::symlink_metadata(parent)
        .map_err(|error| io_error("inspect workspace operation journal directory", error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(corrupt("workspace journal path is not a real directory"));
    }
    Ok(parent.to_path_buf())
}

fn durable_write(file: &mut File, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes)
        .map_err(|error| io_error("write workspace operation journal", error))?;
    file.sync_all()
        .map_err(|error| io_error("sync workspace operation journal", error))
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error("sync workspace operation journal directory", error))
}

fn journal_directory(state_root: &Path) -> PathBuf {
    state_root.join("workspace-operations")
}

fn journal_path(state_root: &Path, operation_id: &str) -> PathBuf {
    let digest = Sha256::digest(operation_id.as_bytes());
    let name = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    journal_directory(state_root).join(format!("{name}.json"))
}

fn failure(error: &RuntimeError) -> StoredFailure {
    StoredFailure {
        code: error.code.clone(),
        detail: error.detail.clone(),
        retryable: error.retryable,
    }
}

fn stored_error(record: &Record) -> Result<RuntimeError> {
    record
        .failure
        .as_ref()
        .map(|failure| {
            RuntimeError::recorded(
                failure.code.clone(),
                failure.detail.clone(),
                failure.retryable,
            )
        })
        .ok_or_else(|| corrupt("terminal workspace operation has no failure"))
}

fn corrupt(detail: &str) -> RuntimeError {
    RuntimeError::rejected("workspace_journal_corrupt", detail)
}

fn io_error(operation: &'static str, error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable(
        "workspace_journal_io_failed",
        format!("{operation}: {error}"),
    )
}
