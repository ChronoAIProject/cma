//! Shared semantic validation of V1 payloads, independent of their storage.
use serde::{de::DeserializeOwned, Serialize};

use super::{validation, Operation, Record, Status, JOURNAL_VERSION};
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};

impl Record {
    pub(crate) fn inventory_name(&self, config: &RuntimeConfig) -> std::path::PathBuf {
        super::io::journal_path(
            &crate::mutation_scope::root(config, &self.subject),
            &self.operation_id,
        )
    }

    pub(crate) fn inventory_replacement(&self, replacement: &Self) -> Result<()> {
        super::io::validate_replacement(self, replacement)
    }

    pub(crate) fn started(config: &RuntimeConfig, operation: &Operation) -> Result<Self> {
        let record = Self {
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
        record.validate(config)?;
        Ok(record)
    }

    pub(crate) fn validate(&self, config: &RuntimeConfig) -> Result<()> {
        if self.sandbox_id != config.sandbox_id {
            return Err(super::corrupt(
                "runtime mutation journal belongs to another sandbox",
            ));
        }
        validation::validate_record(self, config)
    }

    pub(crate) fn decode(bytes: &[u8], config: &RuntimeConfig) -> Result<Self> {
        if bytes.len() as u64 > super::MAX_JOURNAL_BYTES {
            return Err(super::corrupt(
                "runtime mutation journal entry is oversized",
            ));
        }
        let record: Self = serde_json::from_slice(bytes)
            .map_err(|_| super::corrupt("runtime mutation journal payload is invalid"))?;
        record.validate(config)?;
        Ok(record)
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        super::encode_record(self)
    }
    pub(crate) fn status(&self) -> Status {
        self.status
    }
    pub(crate) fn exact(&self, operation: &Operation) -> Result<()> {
        super::ensure_exact(self, operation)
    }
    pub(crate) fn receipt<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(
            self.receipt
                .clone()
                .ok_or_else(|| super::corrupt("completed runtime mutation has no receipt"))?,
        )
        .map_err(|_| super::corrupt("runtime mutation receipt is invalid"))
    }
    pub(crate) fn error(&self) -> Result<RuntimeError> {
        super::stored_error(self)
    }

    pub(crate) fn completed<T: Serialize>(mut self, receipt: &T) -> Result<Self> {
        self.status = Status::Completed;
        self.receipt = Some(
            serde_json::to_value(receipt)
                .map_err(|_| super::corrupt("runtime mutation receipt is invalid"))?,
        );
        self.failure = None;
        Ok(self)
    }

    pub(crate) fn failed(mut self, error: &RuntimeError, indeterminate: bool) -> Result<Self> {
        self.status = if indeterminate {
            Status::Indeterminate
        } else {
            Status::Failed
        };
        self.receipt = None;
        self.failure = Some(super::stored_failure(error));
        super::validate_terminal_shape(&self)?;
        Ok(self)
    }
}
