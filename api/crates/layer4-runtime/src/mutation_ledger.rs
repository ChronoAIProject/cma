//! Indexed authority, private to the runtime. Serving integration is explicit.
mod authority;
mod fence;
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
                for operation in &operations {
                    authority::admit(&transaction, &config, fence, operation)?;
                }
                transaction.commit().map_err(|_| storage_error())
            })
            .await
    }

    pub(crate) async fn close(self) -> Result<()> {
        self.storage.close().await
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
    let ledger = Ledger::open(directory.to_owned(), config.clone(), true, 256).await?;
    ledger.admit(u64::MAX, vec![operation.clone()]).await?;
    ledger.close().await?;
    let ledger = Ledger::open(directory.to_owned(), config, false, 256).await?;
    ledger.admit(u64::MAX, vec![operation.clone()]).await?;
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
