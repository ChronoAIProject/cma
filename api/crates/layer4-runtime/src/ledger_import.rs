//! Offline tools never initialize a home, recover a journal or authorize effects.
//! Inventory is evidence, not proof that a legacy writer has stopped.
mod inventory;
mod io;
mod json;
mod plan;
mod plan_io;
mod sealed;
mod stage;
#[cfg(test)]
mod tests;
mod validation;

use std::time::{Duration, Instant};

use crate::failure::{Result, RuntimeError};

pub(crate) async fn inspect_command() -> std::result::Result<(), String> {
    let config = crate::config::RuntimeConfig::from_environment()
        .map_err(|_| "runtime ledger inspection configuration is invalid".to_owned())?;
    let summary = tokio::task::spawn_blocking(move || {
        inventory::scan(&config, Limits::default()).map(|inventory| inventory.summary())
    })
    .await
    .map_err(|_| "runtime ledger inspection worker failed".to_owned())?
    .map_err(|error| error.to_string())?;
    println!("{summary}");
    Ok(())
}

pub(crate) async fn validate_command() -> std::result::Result<(), String> {
    let config = crate::config::RuntimeConfig::from_environment()
        .map_err(|_| "runtime ledger inspection configuration is invalid".to_owned())?;
    let summary = tokio::task::spawn_blocking(move || {
        let inventory = inventory::scan(&config, Limits::default())?;
        validation::analyze(&config, &inventory).map(|analysis| analysis.summary(&inventory))
    })
    .await
    .map_err(|_| "runtime ledger inspection worker failed".to_owned())?
    .map_err(|error| error.to_string())?;
    println!("{summary}");
    Ok(())
}

pub(crate) async fn plan_command(
    destination: std::path::PathBuf,
) -> std::result::Result<(), String> {
    let config = crate::config::RuntimeConfig::from_environment()
        .map_err(|_| "runtime ledger inspection configuration is invalid".to_owned())?;
    let summary = tokio::task::spawn_blocking(move || plan::create(&config, &destination))
        .await
        .map_err(|_| "runtime ledger inspection worker failed".to_owned())?
        .map_err(|error| error.to_string())?;
    println!("{summary}");
    Ok(())
}

pub(crate) async fn verify_plan_command(
    path: std::path::PathBuf,
    expected: String,
) -> std::result::Result<(), String> {
    let config = crate::config::RuntimeConfig::from_environment()
        .map_err(|_| "runtime ledger inspection configuration is invalid".to_owned())?;
    let summary = tokio::task::spawn_blocking(move || {
        let (plan, analysis) = sealed::open(&config, &path, &expected)?;
        Ok::<_, RuntimeError>(analysis.summary(&plan.inventory))
    })
    .await
    .map_err(|_| "runtime ledger inspection worker failed".to_owned())?
    .map_err(|error| error.to_string())?;
    println!("{summary}");
    Ok(())
}

pub(crate) async fn stage_command(
    plan: std::path::PathBuf,
    expected: String,
    destination: std::path::PathBuf,
) -> std::result::Result<(), String> {
    let config = crate::config::RuntimeConfig::from_environment()
        .map_err(|_| "runtime ledger inspection configuration is invalid".to_owned())?;
    let runtime = tokio::runtime::Handle::current();
    let summary = tokio::task::spawn_blocking(move || {
        runtime.block_on(stage::create(config, &plan, &expected, &destination))
    })
    .await
    .map_err(|_| "runtime ledger inspection worker failed".to_owned())?
    .map_err(|error| error.to_string())?;
    println!("{summary}");
    Ok(())
}

#[derive(Clone, Copy)]
struct Limits {
    entries: usize,
    bytes: u64,
    metadata_bytes: usize,
    deadline: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: 65_536,
            bytes: 2 * 1024 * 1024 * 1024,
            // Leave room below the 64 MiB working budget for a bounded record,
            // decoding and the SQLite cache. No raw history is held in memory.
            metadata_bytes: 24 * 1024 * 1024,
            deadline: Duration::from_secs(300),
        }
    }
}

struct Budget {
    limits: Limits,
    start: Instant,
    entries: usize,
    bytes: u64,
    metadata_bytes: usize,
}

impl Budget {
    fn new(limits: Limits) -> Self {
        Self {
            limits,
            start: Instant::now(),
            entries: 0,
            bytes: 0,
            metadata_bytes: 0,
        }
    }
    fn check(&self) -> Result<()> {
        if self.start.elapsed() > self.limits.deadline {
            return Err(capacity());
        }
        Ok(())
    }
    fn remaining_limits(&self) -> Result<Limits> {
        self.check()?;
        Ok(Limits {
            deadline: self.limits.deadline.saturating_sub(self.start.elapsed()),
            ..self.limits
        })
    }
    fn entry(&mut self, name_bytes: usize) -> Result<()> {
        self.check()?;
        self.entries = self.entries.checked_add(1).ok_or_else(capacity)?;
        self.metadata_bytes = self
            .metadata_bytes
            .checked_add(name_bytes * 2 + 512)
            .ok_or_else(capacity)?;
        if self.entries > self.limits.entries || self.metadata_bytes > self.limits.metadata_bytes {
            return Err(capacity());
        }
        Ok(())
    }
    fn path(&mut self, bytes: usize) -> Result<()> {
        self.metadata_bytes = self
            .metadata_bytes
            .checked_add(bytes * 2 + 512)
            .ok_or_else(capacity)?;
        if self.metadata_bytes > self.limits.metadata_bytes {
            return Err(capacity());
        }
        self.check()
    }
    fn file(&mut self, bytes: u64) -> Result<()> {
        self.check()?;
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(capacity)?;
        if self.bytes > self.limits.bytes {
            return Err(capacity());
        }
        Ok(())
    }
}

fn capacity() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_migration_capacity",
        "runtime mutation ledger migration budget is exhausted",
    )
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_migration_invalid",
        "runtime mutation ledger migration input requires verification",
    )
}

fn changed() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_migration_changed",
        "runtime mutation ledger migration input changed during inspection",
    )
}
