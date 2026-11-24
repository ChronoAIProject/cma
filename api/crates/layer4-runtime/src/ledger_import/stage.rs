//! Detached, create-only import. No source-root writes or release capability.
use std::path::Path;

use super::plan_io::Archive;
use super::{changed, invalid, sealed, Budget, Limits};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_journal::{Record, Subject};
use crate::mutation_ledger::import::HeldImport;

pub(super) const PAGES: u32 = 1024 * 1024 * 1024 / 4096;

#[derive(Clone, Copy)]
pub(super) enum Boundary {
    Database,
    Authority,
    Journal,
    Verified,
    Closed,
    Ready,
}

pub(super) async fn create(
    config: RuntimeConfig,
    plan: &Path,
    expected: &str,
    destination: &Path,
) -> Result<serde_json::Value> {
    create_checked(config, plan, expected, destination, PAGES, |_| Ok(())).await
}

pub(super) async fn create_checked(
    config: RuntimeConfig,
    plan_path: &Path,
    expected: &str,
    destination: &Path,
    pages: u32,
    mut boundary: impl FnMut(Boundary) -> Result<()>,
) -> Result<serde_json::Value> {
    let mut budget = Budget::new(Limits::default());
    if destination.starts_with(plan_path) || plan_path.starts_with(destination) {
        return Err(invalid());
    }
    let (plan, analysis) =
        sealed::open_with_limits(&config, plan_path, expected, budget.remaining_limits()?)?;
    budget.check()?;
    let archive = Archive::create_protected(&config, destination, Some(plan_path))?;
    let database = archive.database()?;
    let directory = std::sync::Arc::new(
        database
            .0
            .try_clone()
            .map_err(|_| super::plan_io::storage())?,
    );
    let store = HeldImport::create_anchored(directory, config.clone(), pages).await?;
    let initialized = async {
        let original_directory = database.identity()?;
        let original_file = database.file_identity("ledger.sqlite3")?;
        let expected_processes = fill(
            &store,
            &config,
            &plan,
            &analysis,
            &mut budget,
            &mut boundary,
        )
        .await?;
        Ok((original_directory, original_file, expected_processes))
    }
    .await;
    let (original_directory, original_file, expected_processes) = match initialized {
        Ok(verified) => verified,
        Err(error) => return store.abort(error).await,
    };
    let mut summary = store.finish(expected_processes).await?;
    let inventory_digest = plan.inventory.digest.clone();
    drop(analysis);
    drop(plan);
    archive.verify_database(&database)?;
    let closed_directory = database.identity()?;
    let closed_file = database.file_identity("ledger.sqlite3")?;
    if (original_directory.device, original_directory.inode)
        != (closed_directory.device, closed_directory.inode)
        || (original_file.device, original_file.inode) != (closed_file.device, closed_file.inode)
    {
        return Err(changed());
    }
    boundary(Boundary::Closed)?;
    for name in database.names(&mut budget)? {
        database.sync_file(&name)?;
    }
    database
        .0
        .sync_all()
        .map_err(|_| super::plan_io::storage())?;
    archive.verify()?;
    let mut verification = Budget::new(budget.remaining_limits()?);
    let database_hash =
        database.hash_file("ledger.sqlite3", u64::from(pages) * 4096, &mut verification)?;
    boundary(Boundary::Ready)?;
    // A ready artifact must still have its complete exact byte backup. This
    // revalidation is content proof only; it never grants live-source exclusion.
    sealed::open_with_limits(&config, plan_path, expected, budget.remaining_limits()?)?;
    archive.verify_database(&database)?;
    if database.identity()? != closed_directory
        || database.file_identity("ledger.sqlite3")? != closed_file
    {
        return Err(changed());
    }
    budget.check()?;
    archive.json(
        "ready.json",
        &serde_json::json!({
            "format":"runtime-ledger-import-ready-v1", "admission":"import_held",
            "writer_exclusion":"unproved", "plan_sha256":expected,
            "inventory_sha256":inventory_digest, "database_sha256":database_hash,
            "summary":summary,
        }),
    )?;
    summary["format"] = "runtime-ledger-import-ready-v1".into();
    summary["writer_exclusion"] = "unproved".into();
    summary["plan_sha256"] = expected.into();
    summary["database_sha256"] = database_hash.into();
    Ok(summary)
}

async fn fill(
    store: &HeldImport,
    config: &RuntimeConfig,
    plan: &super::plan::Plan,
    analysis: &super::validation::Analysis,
    budget: &mut Budget,
    boundary: &mut impl FnMut(Boundary) -> Result<()>,
) -> Result<u64> {
    boundary(Boundary::Database)?;
    for ((scope, key), survivors) in &analysis.authority {
        let mut rows = [None, None, None];
        for (slot, index) in survivors.iter().enumerate() {
            if let Some(index) = index {
                rows[slot] = Some(plan.inventory.read(*index, budget)?);
            }
        }
        store.authority(*scope, key.clone(), rows).await?;
        boundary(Boundary::Authority)?;
    }
    let mut processes = Vec::new();
    for ((scope, _), group) in &analysis.journals {
        let index = group.selected.ok_or_else(invalid)?;
        let bytes = plan.inventory.read(index, budget)?;
        let mut scoped = config.clone();
        if *scope != 0 {
            scoped.generation = *scope;
        }
        let operation = Record::decode(&bytes, &scoped)?.operation();
        if matches!(operation.subject, Subject::OpenAgent { .. })
            || crate::mutation_journal::process_history::uses_incarnation(
                &operation.subject,
                &operation.operation_id,
            )
        {
            processes.push((*scope, index));
        }
        store.journal(*scope, bytes).await?;
        boundary(Boundary::Journal)?;
    }
    let mut expected_processes = 0;
    let mut verification = Budget::new(budget.remaining_limits()?);
    for (scope, index) in processes {
        let bytes = plan.inventory.read(index, &mut verification)?;
        expected_processes += u64::from(store.verify_process(scope, bytes).await?);
    }
    boundary(Boundary::Verified)?;
    Ok(expected_processes)
}
