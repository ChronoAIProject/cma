use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::failure::{Result, RuntimeError};

#[path = "workspace_journal_io.rs"]
mod io;
#[path = "workspace_journal_validation.rs"]
mod validation;

const JOURNAL_VERSION: u32 = 1;
pub(super) const MAX_JOURNAL_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const MAX_JOURNAL_ENTRIES: usize = 1_024;
const MAX_FAILURE_CODE_BYTES: usize = 128;
const MAX_FAILURE_DETAIL_CHARS: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Status {
    Started,
    Completed,
    Failed,
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredFailure {
    code: String,
    detail: String,
    retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
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

#[derive(Debug)]
pub(crate) struct Pending {
    pub kind: String,
    pub ticket: Ticket,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn begin<T: DeserializeOwned, R: Serialize>(
    state_root: &Path,
    home: &Path,
    allow_local_git: bool,
    sandbox_id: &str,
    workspace_id: &str,
    kind: &str,
    operation_id: &str,
    request_sha256: &str,
    request: &R,
) -> Result<Begin<T>> {
    validate_identity(workspace_id, kind, operation_id, request_sha256)?;
    if kind != "remove" {
        require_present(state_root, home, allow_local_git, sandbox_id, workspace_id)?;
    }
    let request = serde_json::to_value(request).map_err(|error| {
        RuntimeError::rejected(
            "workspace_request_invalid",
            format!("encode workspace operation request: {error}"),
        )
    })?;
    let path = io::journal_path(state_root, operation_id);
    match io::read_record(&path)? {
        Some(record) => resolve_existing(
            path,
            record,
            home,
            allow_local_git,
            sandbox_id,
            workspace_id,
            kind,
            operation_id,
            request_sha256,
            &request,
        ),
        None => create_started(
            state_root,
            home,
            allow_local_git,
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
    home: &Path,
    allow_local_git: bool,
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
    validate_stored(&record, home, allow_local_git)?;
    let ticket = Ticket {
        path: io::journal_path(state_root, operation_id),
        record,
    };
    match ticket.persist_new() {
        Ok(()) => Ok(Begin::New(ticket)),
        Err(error) if error.code == "workspace_journal_raced" => {
            let record = io::read_record(&ticket.path)?
                .ok_or_else(|| corrupt("raced workspace journal disappeared"))?;
            resolve_existing(
                ticket.path,
                record,
                home,
                allow_local_git,
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

pub(crate) fn started(
    state_root: &Path,
    sandbox_id: &str,
    home: &Path,
    allow_local_git: bool,
) -> Result<Vec<Pending>> {
    let files = io::scan(state_root)?;
    for file in &files {
        validate_stored(&file.record, home, allow_local_git)?;
        if file.record.sandbox_id != sandbox_id {
            return Err(corrupt(
                "workspace operation journal belongs to another sandbox",
            ));
        }
    }
    let records = io::reconcile(files)?;
    let mut pending = Vec::new();
    for file in records
        .into_iter()
        .filter(|file| file.record.status == Status::Started)
    {
        let ticket = Ticket {
            path: file.path,
            record: file.record,
        };
        if ticket.record.kind != "remove" {
            if let Err(error) = require_present(
                state_root,
                home,
                allow_local_git,
                sandbox_id,
                &ticket.record.workspace_id,
            ) {
                if error.code != "workspace_removed" {
                    return Err(error);
                }
                ticket.fail(&error)?;
                continue;
            }
        }
        pending.push(Pending {
            kind: ticket.record.kind.clone(),
            ticket,
        });
    }
    Ok(pending)
}

// The removal intent is a permanent barrier, including before its I/O completes.
fn require_present(
    state_root: &Path,
    home: &Path,
    allow_local_git: bool,
    sandbox_id: &str,
    workspace_id: &str,
) -> Result<()> {
    let operation_id = format!("workspace:{workspace_id}:remove:v1");
    let Some(record) = io::read_record(&io::journal_path(state_root, &operation_id))? else {
        return Ok(());
    };
    validate_stored(&record, home, allow_local_git)?;
    if record.sandbox_id != sandbox_id
        || record.operation_id != operation_id
        || record.kind != "remove"
    {
        return Err(corrupt(
            "workspace removal barrier belongs to another operation",
        ));
    }
    Err(RuntimeError::rejected(
        "workspace_removed",
        "workspace removal was admitted; preparation and setup cannot resume",
    ))
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
        if error.retryable {
            return Err(corrupt(
                "retryable workspace failure cannot become a terminal journal record",
            ));
        }
        self.record.status = Status::Failed;
        self.record.receipt = None;
        self.record.failure = Some(failure(error));
        self.persist_atomic()
    }

    pub(crate) fn indeterminate(mut self, detail: &str) -> Result<RuntimeError> {
        let code = indeterminate_code(&self.record.kind)?;
        let error = RuntimeError::rejected(code, detail);
        self.record.status = Status::Indeterminate;
        self.record.receipt = None;
        self.record.failure = Some(failure(&error));
        self.persist_atomic()?;
        Ok(error)
    }

    fn persist_new(&self) -> Result<()> {
        io::persist_new(&self.path, &encode_record(&self.record)?)
    }

    fn persist_atomic(&self) -> Result<()> {
        io::persist_atomic(&self.path, &encode_record(&self.record)?)
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_existing<T: DeserializeOwned>(
    path: PathBuf,
    record: Record,
    home: &Path,
    allow_local_git: bool,
    sandbox_id: &str,
    workspace_id: &str,
    kind: &str,
    operation_id: &str,
    request_sha256: &str,
    request: &serde_json::Value,
) -> Result<Begin<T>> {
    validate_stored(&record, home, allow_local_git)?;
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
        Status::Completed => serde_json::from_value(
            ticket
                .record
                .receipt
                .ok_or_else(|| corrupt("completed workspace operation has no receipt"))?,
        )
        .map(Begin::Completed)
        .map_err(|error| corrupt(&format!("decode workspace operation receipt: {error}"))),
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
    let credential_setup =
        kind == "setup" && operation_id == format!("workspace:{workspace_id}:credential-setup:v1");
    if (operation_id != expected && !credential_setup)
        || workspace_id.is_empty()
        || workspace_id.len() > 128
        || !workspace_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        || !validation::lower_sha256(request_sha256)
    {
        return Err(RuntimeError::rejected(
            "workspace_operation_invalid",
            "workspace operation identity is invalid",
        ));
    }
    Ok(())
}

fn validate_stored(record: &Record, home: &Path, allow_local_git: bool) -> Result<()> {
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
    )
    .map_err(|_| corrupt("workspace journal operation identity is invalid"))?;
    validation::validate_request_and_receipt(record, home, allow_local_git)?;
    match record.status {
        Status::Started if record.receipt.is_none() && record.failure.is_none() => Ok(()),
        Status::Completed if record.receipt.is_some() && record.failure.is_none() => Ok(()),
        Status::Failed if record.receipt.is_none() => {
            validate_failure(record.failure.as_ref(), None)
        }
        Status::Indeterminate if record.receipt.is_none() => validate_failure(
            record.failure.as_ref(),
            Some(indeterminate_code(&record.kind)?),
        ),
        _ => Err(corrupt("workspace journal terminal shape is invalid")),
    }
}

fn validate_failure(failure: Option<&StoredFailure>, expected_code: Option<&str>) -> Result<()> {
    let Some(failure) = failure else {
        return Err(corrupt("terminal workspace operation has no failure"));
    };
    let valid_code = !failure.code.is_empty()
        && failure.code.len() <= MAX_FAILURE_CODE_BYTES
        && failure
            .code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    let valid_detail = !failure.detail.is_empty()
        && failure.detail.chars().count() <= MAX_FAILURE_DETAIL_CHARS
        && failure
            .detail
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\n' | '\t'));
    if !valid_code
        || !valid_detail
        || failure.retryable
        || expected_code.is_some_and(|expected| failure.code != expected)
    {
        return Err(corrupt(
            "workspace journal failure is not a typed terminal failure",
        ));
    }
    Ok(())
}

fn indeterminate_code(kind: &str) -> Result<&'static str> {
    match kind {
        "prepare" => Ok("workspace_prepare_outcome_unknown"),
        "setup" => Ok("workspace_setup_outcome_unknown"),
        "remove" => Ok("workspace_remove_outcome_unknown"),
        _ => Err(corrupt(
            "workspace journal contains an unknown operation kind",
        )),
    }
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

pub(super) fn corrupt(detail: &str) -> RuntimeError {
    RuntimeError::rejected("workspace_journal_corrupt", detail)
}

pub(super) fn io_error(operation: &'static str, error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable(
        "workspace_journal_io_failed",
        format!("{operation}: {error}"),
    )
}

/// Read-only proof of the one recoverable original outcome. Recovery never
/// rewrites, completes or replaces this original admission journal.
pub(crate) fn indeterminate_prepare_digest(
    config: &crate::config::RuntimeConfig,
    request: &layer3_adapter::WorkspaceRequest,
) -> Result<String> {
    use sha2::{Digest, Sha256};
    require_present(
        &config.state_root,
        &config.home,
        config.allow_local_git,
        &config.sandbox_id,
        &request.workspace_id,
    )?;
    let bytes = io::read_bytes(&io::journal_path(&config.state_root, &request.operation_id))?
        .ok_or_else(|| corrupt("original Workspace preparation journal is absent"))?;
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|_| corrupt("original Workspace journal is invalid"))?;
    validate_stored(&record, &config.home, config.allow_local_git)?;
    if record.sandbox_id != config.sandbox_id
        || record.workspace_id != request.workspace_id
        || record.operation_id != request.operation_id
        || record.request_sha256 != request.request_sha256
        || record.request
            != serde_json::to_value(request).map_err(|_| corrupt("invalid original request"))?
        || record.kind != "prepare"
        || record.status != Status::Indeterminate
        || record
            .failure
            .as_ref()
            .is_none_or(|failure| failure.code != "workspace_prepare_outcome_unknown")
    {
        return Err(RuntimeError::rejected(
            "workspace_recovery_ineligible",
            "only the exact indeterminate original Workspace preparation can be recovered",
        ));
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
#[path = "workspace_journal_tests.rs"]
mod tests;
