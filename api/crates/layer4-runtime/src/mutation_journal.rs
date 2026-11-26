use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};

#[path = "mutation_journal_codec.rs"]
mod codec;

#[path = "mutation_journal_io.rs"]
mod io;
#[path = "mutation_journal_lookup.rs"]
mod lookup;
#[path = "mutation_operation.rs"]
mod operation;
#[path = "mutation_process_history.rs"]
pub(crate) mod process_history;
#[path = "mutation_journal_validation.rs"]
mod validation;
pub(crate) use operation::{ActiveAgent, Operation, Subject};

const JOURNAL_VERSION: u32 = 1;
pub(super) const MAX_JOURNAL_BYTES: u64 = 1024 * 1024;
pub(super) const MAX_JOURNAL_ENTRIES: usize = 1_024;
const MAX_FAILURE_CODE_BYTES: usize = 128;
const MAX_FAILURE_DETAIL_CHARS: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_generation: Option<u64>,
    operation_id: String,
    request_sha256: String,
    payload_sha256: String,
    subject: Subject,
    status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    receipt: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failure: Option<StoredFailure>,
}

impl Record {
    pub(crate) fn operation(&self) -> Operation {
        Operation {
            operation_id: self.operation_id.clone(),
            request_sha256: self.request_sha256.clone(),
            payload_sha256: self.payload_sha256.clone(),
            subject: self.subject.clone(),
        }
    }
}

#[derive(Debug)]
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
    pub operation: Operation,
    pub ticket: Ticket,
}

pub(crate) fn admitted(config: &RuntimeConfig, operation: &Operation) -> Result<bool> {
    Ok(read_exact(config, operation)?.is_some())
}

pub(crate) fn completed(config: &RuntimeConfig, operation: &Operation) -> Result<bool> {
    Ok(read_exact(config, operation)?.is_some_and(|record| record.status == Status::Completed))
}

pub(crate) fn find_operation(
    config: &RuntimeConfig,
    operation_id: &str,
    scope_subject: &Subject,
) -> Result<Option<Record>> {
    lookup::find_by_id(config, operation_id, scope_subject)
}

pub(crate) fn find_proof_record(
    config: &RuntimeConfig,
    operation_id: &str,
    scope_subject: &Subject,
) -> Result<Option<Record>> {
    lookup::find_for_proof(config, operation_id, scope_subject)
}

fn read_exact(config: &RuntimeConfig, operation: &Operation) -> Result<Option<Record>> {
    validation::validate_operation(operation, config)?;
    let Some(record) = lookup::find(config, operation)? else {
        return Ok(None);
    };
    ensure_exact(&record, operation)?;
    Ok(Some(record))
}

pub(crate) fn validate_operation(config: &RuntimeConfig, operation: &Operation) -> Result<()> {
    validation::validate_operation(operation, config)
}

pub(crate) fn begin<T: DeserializeOwned>(
    config: &RuntimeConfig,
    operation: &Operation,
) -> Result<Begin<T>> {
    validation::validate_operation(operation, config)?;
    let path = io::journal_path(
        &crate::mutation_scope::root(config, &operation.subject),
        &operation.operation_id,
    );
    if let Some(record) = lookup::find(config, operation)? {
        return resolve_existing(path, record, operation);
    }
    crate::mutation_capacity::check(config, std::slice::from_ref(operation))?;
    create_started(config, operation)
}

fn create_started<T: DeserializeOwned>(
    config: &RuntimeConfig,
    operation: &Operation,
) -> Result<Begin<T>> {
    let record = Record {
        version: JOURNAL_VERSION,
        sandbox_id: config.sandbox_id.clone(),
        runtime_generation: crate::mutation_scope::generation(config, &operation.subject),
        operation_id: operation.operation_id.clone(),
        request_sha256: operation.request_sha256.clone(),
        payload_sha256: operation.payload_sha256.clone(),
        subject: operation.subject.clone(),
        status: Status::Started,
        receipt: None,
        failure: None,
    };
    validation::validate_record(&record, config)?;
    let ticket = Ticket {
        path: io::journal_path(
            &crate::mutation_scope::root(config, &operation.subject),
            &operation.operation_id,
        ),
        record,
    };
    match ticket.persist_new() {
        Ok(()) => Ok(Begin::New(ticket)),
        Err(error) if error.code == "mutation_journal_raced" => begin(config, operation),
        Err(error) => Err(error),
    }
}

pub(crate) fn started(config: &RuntimeConfig) -> Result<Vec<Pending>> {
    Ok(records(config)?
        .into_iter()
        .filter(|record| record.status == Status::Started)
        .map(|record| {
            let operation = record.operation();
            Pending {
                ticket: Ticket {
                    path: io::journal_path(
                        &crate::mutation_scope::root(config, &record.subject),
                        &record.operation_id,
                    ),
                    record,
                },
                operation,
            }
        })
        .collect())
}

#[cfg(test)]
pub(crate) fn validate_all(config: &RuntimeConfig) -> Result<()> {
    records(config).map(|_| ())
}

pub(crate) fn active_agents(config: &RuntimeConfig) -> Result<Vec<ActiveAgent>> {
    use layer3_adapter::AgentProcessAction::{Close, Open};

    let records = records(config)?;
    let closed = records
        .iter()
        .filter(|record| record.status == Status::Completed)
        .filter_map(|record| match &record.subject {
            Subject::CloseAgent { .. } => Some(record.operation_id.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut active = BTreeMap::new();
    for record in records {
        let Subject::OpenAgent {
            agent_id,
            cwd,
            home,
        } = record.subject
        else {
            continue;
        };
        let incarnation = Open
            .incarnation(&agent_id, &record.operation_id)
            .ok_or_else(|| corrupt("runtime journal contains an invalid process identity"))?;
        if record.status != Status::Failed
            && !closed.contains(&Close.operation_id(&agent_id, incarnation))
            && active
                .insert(
                    agent_id.clone(),
                    ActiveAgent {
                        agent_id,
                        cwd,
                        home,
                    },
                )
                .is_some()
        {
            return Err(corrupt(
                "runtime journal contains multiple active process incarnations",
            ));
        }
    }
    Ok(active.into_values().collect())
}

impl Ticket {
    pub(crate) fn complete<T: Serialize>(mut self, receipt: &T) -> Result<()> {
        self.record.status = Status::Completed;
        self.record.receipt = Some(serde_json::to_value(receipt).map_err(|error| {
            RuntimeError::unavailable(
                "mutation_journal_io_failed",
                format!("encode runtime mutation receipt: {error}"),
            )
        })?);
        self.record.failure = None;
        self.persist_atomic()
    }

    pub(crate) fn fail(mut self, error: &RuntimeError) -> Result<()> {
        if error.retryable {
            return Err(corrupt(
                "retryable runtime mutation failure cannot become terminal",
            ));
        }
        self.record.status = Status::Failed;
        self.record.receipt = None;
        self.record.failure = Some(stored_failure(error));
        self.persist_atomic()
    }

    pub(crate) fn supersede(self) -> Result<()> {
        self.fail(&RuntimeError::rejected(
            "mutation_authority_superseded",
            "runtime mutation was superseded by newer resource authority",
        ))
    }

    pub(crate) fn indeterminate(mut self, detail: &str) -> Result<RuntimeError> {
        let error = RuntimeError::rejected(
            match self.record.subject {
                Subject::OpenAgent { .. } => "agent_open_outcome_unknown",
                Subject::CloseAgent { .. } => "agent_close_outcome_unknown",
                _ => "runtime_mutation_outcome_unknown",
            },
            detail,
        );
        self.record.status = Status::Indeterminate;
        self.record.receipt = None;
        self.record.failure = Some(stored_failure(&error));
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

pub(crate) fn inventory(config: &RuntimeConfig) -> Result<Vec<Operation>> {
    read_records(config, false).map(|records| records.iter().map(Record::operation).collect())
}

fn records(config: &RuntimeConfig) -> Result<Vec<Record>> {
    crate::mutation_capacity::check_recovery(config)?;
    read_records(config, true)
}

fn read_records(config: &RuntimeConfig, apply: bool) -> Result<Vec<Record>> {
    let mut files = Vec::new();
    let mut budget = crate::mutation_recovery::ScanBudget::new(
        config,
        crate::mutation_recovery::journal_limit(config),
    );
    for root in crate::mutation_scope::roots(config)? {
        files.extend(io::scan(&root, &mut budget)?);
    }
    for file in &files {
        validation::validate_record(&file.record, config)?;
        if file.record.sandbox_id != config.sandbox_id {
            return Err(corrupt(
                "runtime mutation journal belongs to another sandbox",
            ));
        }
        if file.path.parent()
            != io::journal_path(
                &crate::mutation_scope::root(config, &file.record.subject),
                &file.record.operation_id,
            )
            .parent()
        {
            return Err(corrupt(
                "runtime mutation journal is in another generation scope",
            ));
        }
    }
    let records: Vec<_> = io::reconcile(files, apply)?
        .into_iter()
        .map(|file| file.record)
        .collect();
    process_history::validate_records(&records)?;
    Ok(records)
}

fn resolve_existing<T: DeserializeOwned>(
    path: PathBuf,
    record: Record,
    operation: &Operation,
) -> Result<Begin<T>> {
    ensure_exact(&record, operation)?;
    let ticket = Ticket { path, record };
    match ticket.record.status {
        Status::Started => Ok(Begin::Started(ticket)),
        Status::Completed => serde_json::from_value(
            ticket
                .record
                .receipt
                .ok_or_else(|| corrupt("completed runtime mutation has no receipt"))?,
        )
        .map(Begin::Completed)
        .map_err(|error| corrupt(&format!("decode runtime mutation receipt: {error}"))),
        Status::Failed => Ok(Begin::Failed(stored_error(&ticket.record)?)),
        Status::Indeterminate => Ok(Begin::Indeterminate(stored_error(&ticket.record)?)),
    }
}

fn ensure_exact(record: &Record, operation: &Operation) -> Result<()> {
    if record.operation() != *operation {
        return Err(RuntimeError::rejected(
            "runtime_mutation_replay_mismatch",
            "runtime mutation replay differs from the admitted request or rendered payload",
        ));
    }
    Ok(())
}

fn validate_failure(failure: Option<&StoredFailure>, expected_code: Option<&str>) -> Result<()> {
    let Some(failure) = failure else {
        return Err(corrupt("terminal runtime mutation has no failure"));
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
            "runtime mutation journal failure is not a typed terminal failure",
        ));
    }
    Ok(())
}

pub(super) fn validate_terminal_shape(record: &Record) -> Result<()> {
    match record.status {
        Status::Started if record.receipt.is_none() && record.failure.is_none() => Ok(()),
        Status::Completed if record.receipt.is_some() && record.failure.is_none() => Ok(()),
        Status::Failed if record.receipt.is_none() => {
            validate_failure(record.failure.as_ref(), None)
        }
        Status::Indeterminate if record.receipt.is_none() => validate_failure(
            record.failure.as_ref(),
            Some(match record.subject {
                Subject::OpenAgent { .. } => "agent_open_outcome_unknown",
                Subject::CloseAgent { .. } => "agent_close_outcome_unknown",
                _ => "runtime_mutation_outcome_unknown",
            }),
        ),
        _ => Err(corrupt(
            "runtime mutation journal terminal shape is invalid",
        )),
    }
}

fn encode_record(record: &Record) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(record).map_err(|error| {
        RuntimeError::unavailable(
            "mutation_journal_io_failed",
            format!("encode runtime mutation journal: {error}"),
        )
    })?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(corrupt("runtime mutation journal entry is oversized"));
    }
    Ok(bytes)
}

fn stored_failure(error: &RuntimeError) -> StoredFailure {
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
        .ok_or_else(|| corrupt("terminal runtime mutation has no failure"))
}

pub(super) fn corrupt(detail: &str) -> RuntimeError {
    RuntimeError::rejected("mutation_journal_corrupt", detail)
}

pub(super) fn io_error(operation: &'static str, error: std::io::Error) -> RuntimeError {
    RuntimeError::unavailable(
        "mutation_journal_io_failed",
        format!("{operation}: {error}"),
    )
}

#[cfg(test)]
#[path = "mutation_journal_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mutation_capacity_tests.rs"]
pub(crate) mod capacity_tests;
