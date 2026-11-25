//! Offline tools never initialize a home, recover a journal or authorize effects.
//! Inventory is evidence, not proof that a legacy writer has stopped.
#[cfg(test)]
mod diagnostics;
pub(crate) mod evidence;
pub(crate) mod inventory;
pub(crate) mod io;
mod isolation;
pub(crate) mod json;
mod plan;
mod plan_io;
mod publication;
mod retirement;
mod retirement_artifact;
pub(crate) mod retirement_move;
mod retirement_stage;
mod sealed;
mod stage;
#[cfg(test)]
mod tests;
mod validation;

use std::time::{Duration, Instant};

use crate::failure::{Result, RuntimeError};

pub(crate) fn cutover_command() -> Result<()> {
    Err(RuntimeError::rejected(
        "mutation_ledger_import_held",
        "live ledger cutover is unavailable until the reviewed maintenance-admission and release-authority protocol in #1461",
    ))
}

pub(crate) async fn inspect_command() -> std::result::Result<(), String> {
    let config = crate::config::RuntimeConfig::from_environment()
        .map_err(|_| "runtime ledger inspection configuration is invalid".to_owned())?;
    let summary = tokio::task::spawn_blocking(move || {
        inventory::scan(&config, Limits::default()).map(|inventory| {
            let mut summary = inventory.summary();
            summary["capacity"] = inventory.capacity_summary(config.generation);
            summary
        })
    })
    .await
    .map_err(|_| "runtime ledger inspection worker failed".to_owned())?
    .map_err(|error| error.to_string())?;
    println!("{summary}");
    Ok(())
}

pub(crate) async fn retirement_preview_command(
    snapshot: std::path::PathBuf,
    digest: String,
) -> std::result::Result<(), String> {
    let config = crate::config::RuntimeConfig::from_environment()
        .map_err(|_| "runtime ledger inspection configuration is invalid".to_owned())?;
    let summary =
        tokio::task::spawn_blocking(move || retirement::preview(&config, &snapshot, &digest))
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

pub(crate) async fn retirement_stage_command(
    plan: std::path::PathBuf,
    plan_sha256: String,
    observation: std::path::PathBuf,
    observation_sha256: String,
    destination: std::path::PathBuf,
) -> std::result::Result<(), String> {
    let config = crate::config::RuntimeConfig::from_environment()
        .map_err(|_| "runtime ledger inspection configuration is invalid".to_owned())?;
    let runtime = tokio::runtime::Handle::current();
    let summary = tokio::task::spawn_blocking(move || {
        runtime.block_on(retirement_stage::create(
            config,
            retirement_stage::Input {
                plan: &plan,
                plan_sha256: &plan_sha256,
                observation: &observation,
                observation_sha256: &observation_sha256,
            },
            &destination,
        ))
    })
    .await
    .map_err(|_| "runtime ledger inspection worker failed".to_owned())?
    .map_err(|error| error.to_string())?;
    println!("{summary}");
    Ok(())
}

pub(crate) async fn retirement_verify_command(
    path: std::path::PathBuf,
    expected: String,
) -> std::result::Result<(), String> {
    let config = crate::config::RuntimeConfig::from_environment()
        .map_err(|_| "runtime ledger inspection configuration is invalid".to_owned())?;
    let summary =
        tokio::task::spawn_blocking(move || retirement_artifact::verify(&config, &path, &expected))
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
pub(crate) struct Limits {
    pub(crate) entries: usize,
    pub(crate) bytes: u64,
    pub(crate) metadata_bytes: usize,
    pub(crate) deadline: Duration,
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

pub(crate) struct Budget {
    limits: Limits,
    start: Instant,
    pub(crate) entries: usize,
    pub(crate) bytes: u64,
    pub(crate) metadata_bytes: usize,
}

impl Budget {
    pub(crate) fn new(limits: Limits) -> Self {
        Self {
            limits,
            start: Instant::now(),
            entries: 0,
            bytes: 0,
            metadata_bytes: 0,
        }
    }
    pub(crate) fn check(&self) -> Result<()> {
        if self.start.elapsed() > self.limits.deadline {
            return Err(capacity_at_least(
                "elapsed_nanoseconds",
                self.start.elapsed().as_nanos(),
            ));
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
        if self.entries > self.limits.entries {
            return Err(capacity_at_least("examined_entries", self.entries as u128));
        }
        if self.metadata_bytes > self.limits.metadata_bytes {
            return Err(capacity_at_least(
                "metadata_bytes",
                self.metadata_bytes as u128,
            ));
        }
        Ok(())
    }
    fn path(&mut self, bytes: usize) -> Result<()> {
        self.metadata_bytes = self
            .metadata_bytes
            .checked_add(bytes * 2 + 512)
            .ok_or_else(capacity)?;
        if self.metadata_bytes > self.limits.metadata_bytes {
            return Err(capacity_at_least(
                "metadata_bytes",
                self.metadata_bytes as u128,
            ));
        }
        self.check()
    }
    fn file(&mut self, bytes: u64) -> Result<()> {
        self.check()?;
        self.bytes = self.bytes.checked_add(bytes).ok_or_else(capacity)?;
        if self.bytes > self.limits.bytes {
            return Err(capacity_at_least(
                "requested_read_bytes",
                self.bytes as u128,
            ));
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

fn capacity_at_least(budget: &'static str, lower_bound: u128) -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_migration_capacity",
        format!("runtime mutation ledger migration budget is exhausted; {budget}_at_least={lower_bound}"),
    )
}

#[track_caller]
fn invalid() -> RuntimeError {
    #[cfg(test)]
    diagnostics::record("invalid", None);
    RuntimeError::rejected(
        "mutation_ledger_migration_invalid",
        "runtime mutation ledger migration input requires verification",
    )
}

#[track_caller]
fn invalid_guard(guard: &'static str) -> RuntimeError {
    #[cfg(test)]
    diagnostics::record(guard, None);
    #[cfg(not(test))]
    let _ = guard;
    invalid()
}

#[track_caller]
fn invalid_io(guard: &'static str, error: std::io::Error) -> RuntimeError {
    #[cfg(test)]
    diagnostics::record(guard, Some(error));
    #[cfg(not(test))]
    let _ = (guard, error);
    invalid()
}

fn changed() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_migration_changed",
        "runtime mutation ledger migration input changed during inspection",
    )
}
