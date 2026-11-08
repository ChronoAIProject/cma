//! Indexed authority, private to the runtime. Serving integration is explicit.
mod authority;
mod budget;
#[cfg(test)]
mod crash_tests;
#[cfg(test)]
mod fault_tests;
mod fence;
pub(crate) mod import;
mod journal;
#[cfg(test)]
mod journal_tests;
#[cfg(test)]
mod lifecycle_tests;
mod observation;
mod processes;
mod queries;
#[cfg(test)]
mod queries_tests;
pub(crate) use queries::ProcessFilter;
#[cfg(test)]
mod processes_tests;
mod schema;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::ledger_storage::Storage;
use crate::mutation_journal::Operation;

pub(crate) struct Ledger {
    storage: Storage,
    config: Arc<RuntimeConfig>,
    directory: PathBuf,
    guard: Option<Arc<crate::ledger_format::Guard>>,
}

impl Ledger {
    pub(crate) async fn open(
        path: PathBuf,
        config: RuntimeConfig,
        create: bool,
        pages: u32,
    ) -> Result<Self> {
        let storage = Storage::open(path.clone(), create, pages).await?;
        let expected = config.clone();
        let storage = storage
            .validate(move |db| schema::open(db, &expected, create))
            .await?;
        Ok(Self {
            storage,
            directory: path,
            guard: None,
            config: Arc::new(config),
        })
    }

    pub(crate) async fn open_guarded(
        path: PathBuf,
        config: RuntimeConfig,
        pages: u32,
        guard: Arc<crate::ledger_format::Guard>,
    ) -> Result<Self> {
        let storage =
            Storage::open_with_lock(path.clone(), pages, Arc::clone(&guard.writer)).await?;
        let expected = config.clone();
        let proof = Arc::clone(&guard);
        let storage = storage
            .validate(move |db| schema::open_guarded(db, &expected, false, Some(&proof)))
            .await?;
        Ok(Self {
            storage,
            directory: path,
            config: Arc::new(config),
            guard: Some(guard),
        })
    }

    pub(crate) fn verify(&self) -> Result<()> {
        if let Some(guard) = &self.guard {
            guard.verify()?;
        }
        Ok(())
    }

    pub(crate) async fn store_id(&self) -> Result<String> {
        self.storage
            .call(|db| {
                let bytes: Vec<u8> = db
                    .query_row(
                        "SELECT store_id FROM metadata WHERE singleton=1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|_| invalid())?;
                uuid::Uuid::from_slice(&bytes)
                    .map(|id| id.to_string())
                    .map_err(|_| invalid())
            })
            .await
    }

    pub(crate) async fn admit(&self, fence: u64, operations: Vec<Operation>) -> Result<()> {
        self.verify()?;
        if operations.is_empty() || operations.len() > 2 || fence == 0 {
            return Err(invalid());
        }
        for operation in &operations {
            crate::mutation_journal::validate_operation(&self.config, operation)?;
        }
        let config = Arc::clone(&self.config);
        let directory = self.directory.clone();
        self.storage
            .call(move |db| {
                let transaction = db.transaction().map_err(|_| storage_error())?;
                let mut new_normal = false;
                for operation in &operations {
                    authority::admit(&transaction, &config, fence, operation)?;
                    new_normal |= journal::reserve(&transaction, &config, operation)?;
                }
                if new_normal {
                    crate::ledger_storage::reserve::require(&directory)?;
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
        self.verify()?;
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
        self.verify()?;
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

#[path = "mutation_ledger/probe.rs"]
mod probe;
pub(crate) use probe::run as probe;

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

#[cfg(test)]
impl Ledger {
    pub(crate) async fn open_fault_vfs(
        path: PathBuf,
        config: RuntimeConfig,
        name: String,
    ) -> Result<Self> {
        let storage = Storage::open_fault_vfs(path.clone(), false, 32_768, name).await?;
        let expected = config.clone();
        let storage = storage
            .validate(move |db| schema::open(db, &expected, false))
            .await?;
        Ok(Self {
            storage,
            config: Arc::new(config),
            directory: path,
            guard: None,
        })
    }

    pub(crate) async fn test_record(
        &self,
        operation: Operation,
    ) -> Result<crate::mutation_journal::Record> {
        let config = Arc::clone(&self.config);
        self.storage
            .call(move |db| {
                journal::load(db, &config, &operation)?
                    .and_then(|stored| stored.record)
                    .ok_or_else(invalid)
            })
            .await
    }
}
