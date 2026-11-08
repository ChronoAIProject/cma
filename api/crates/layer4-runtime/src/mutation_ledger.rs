//! Indexed authority, private to the runtime. Serving integration is explicit.
mod authority;
mod budget;
mod fence;
mod journal;
#[cfg(test)]
mod journal_tests;
mod processes;
#[cfg(test)]
mod processes_tests;
mod schema;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::ledger_storage::Storage;
use crate::mutation_journal::Operation;

pub(crate) struct Ledger {
    storage: Storage,
    config: Arc<RuntimeConfig>,
}

impl Ledger {
    pub(crate) async fn open(
        path: PathBuf,
        config: RuntimeConfig,
        create: bool,
        pages: u32,
    ) -> Result<Self> {
        let storage = Storage::open(path, create, pages).await?;
        let expected = config.clone();
        storage
            .call(move |db| schema::open(db, &expected, create))
            .await?;
        Ok(Self {
            storage,
            config: Arc::new(config),
        })
    }

    pub(crate) async fn admit(&self, fence: u64, operations: Vec<Operation>) -> Result<()> {
        if operations.is_empty() || operations.len() > 2 || fence == 0 {
            return Err(invalid());
        }
        for operation in &operations {
            crate::mutation_journal::validate_operation(&self.config, operation)?;
        }
        let config = Arc::clone(&self.config);
        self.storage
            .call(move |db| {
                let transaction = db.transaction().map_err(|_| storage_error())?;
                let mut new_normal = false;
                for operation in &operations {
                    authority::admit(&transaction, &config, fence, operation)?;
                    new_normal |= journal::reserve(&transaction, &config, operation)?;
                }
                if new_normal {
                    budget::check(&transaction, config.generation)?;
                }
                budget::revision(&transaction)?;
                transaction.commit().map_err(|_| storage_error())
            })
            .await
    }

    pub(crate) async fn close(self) -> Result<()> {
        self.storage.close().await
    }

    pub(crate) async fn begin(&self, operation: Operation) -> Result<journal::Entry> {
        crate::mutation_journal::validate_operation(&self.config, &operation)?;
        let config = Arc::clone(&self.config);
        self.storage
            .call(move |db| {
                let tx = db.transaction().map_err(|_| storage_error())?;
                let entry = journal::begin(&tx, &config, &operation)?;
                budget::revision(&tx)?;
                tx.commit().map_err(|_| storage_error())?;
                Ok(entry)
            })
            .await
    }

    pub(crate) async fn finish(&self, record: crate::mutation_journal::Record) -> Result<()> {
        let config = Arc::clone(&self.config);
        self.storage
            .call(move |db| {
                let tx = db.transaction().map_err(|_| storage_error())?;
                journal::finish(&tx, &config, &record)?;
                budget::revision(&tx)?;
                tx.commit().map_err(|_| storage_error())
            })
            .await
    }
}

pub(crate) async fn probe(directory: &Path) -> Result<()> {
    let config = RuntimeConfig::from_values(std::collections::BTreeMap::from([
        ("CHRONO_SANDBOX_ID".into(), "ledger-storage-probe".into()),
        (
            "HOME".into(),
            directory.with_extension("home").to_string_lossy().into(),
        ),
        (
            "CHRONO_RUNTIME_STATE_ROOT".into(),
            directory.to_string_lossy().into(),
        ),
    ]))
    .map_err(|_| invalid())?;
    let operation = Operation::close_agent(&layer3_adapter::AgentCloseRequest::new(
        "agent:storage-probe:close:v1",
        layer3_adapter::AgentSlot::new("storage-probe"),
    ));
    let ledger = Ledger::open(directory.to_owned(), config.clone(), true, 16_384).await?;
    ledger.admit(u64::MAX, vec![operation.clone()]).await?;
    let entry = ledger.begin(operation.clone()).await?;
    if !entry.new {
        return Err(invalid());
    }
    let receipt = layer3_adapter::wire::AgentCloseReceipt {
        operation_id: operation.operation_id.clone(),
        request_sha256: operation.request_sha256.clone(),
        agent_id: layer3_adapter::AgentSlot::new("storage-probe"),
    };
    ledger.finish(entry.record.completed(&receipt)?).await?;
    ledger.close().await?;
    let ledger = Ledger::open(directory.to_owned(), config, false, 16_384).await?;
    ledger.admit(u64::MAX, vec![operation.clone()]).await?;
    let replay = ledger.begin(operation.clone()).await?;
    if replay.new
        || replay
            .record
            .receipt::<layer3_adapter::wire::AgentCloseReceipt>()?
            != receipt
    {
        return Err(invalid());
    }
    let failure_operation = Operation::close_agent(&layer3_adapter::AgentCloseRequest::new(
        "agent:failure-probe:close:v1",
        layer3_adapter::AgentSlot::new("failure-probe"),
    ));
    ledger.admit(1, vec![failure_operation.clone()]).await?;
    let entry = ledger.begin(failure_operation.clone()).await?;
    ledger
        .finish(entry.record.failed(
            &RuntimeError::rejected("probe_failure", "synthetic failure"),
            false,
        )?)
        .await?;
    if ledger.begin(failure_operation).await?.record.error()?.code != "probe_failure" {
        return Err(invalid());
    }
    let error = ledger
        .admit(u64::MAX - 1, vec![operation])
        .await
        .err()
        .ok_or_else(invalid)?;
    if error.code != "mutation_authority_stale" {
        return Err(invalid());
    }
    ledger.close().await
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_invalid",
        "runtime mutation ledger validation failed",
    )
}

fn storage_error() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_storage_hold",
        "runtime mutation ledger storage requires verification",
    )
}
