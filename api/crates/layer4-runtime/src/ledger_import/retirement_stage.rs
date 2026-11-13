//! Build a detached retirement archive from a pinned byte backup and API
//! observation. Every group stays pending; no active source or format is changed.
use std::path::Path;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use super::inventory::{Inventory, Kind};
use super::plan_io::Archive as Output;
use super::{changed, invalid, retirement, sealed, Budget, Limits};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::retirement_archive::{Archive, GroupProof, SourceRecord};

pub(super) struct Input<'a> {
    pub(super) plan: &'a Path,
    pub(super) plan_sha256: &'a str,
    pub(super) observation: &'a Path,
    pub(super) observation_sha256: &'a str,
}

#[derive(Clone, Copy)]
pub(super) enum Boundary {
    Copy,
    Closed,
    Ready,
}

pub(super) async fn create(
    config: RuntimeConfig,
    input: Input<'_>,
    destination: &Path,
) -> Result<serde_json::Value> {
    create_checked(config, input, destination, |_| Ok(())).await
}

pub(super) async fn create_checked(
    config: RuntimeConfig,
    input: Input<'_>,
    destination: &Path,
    mut boundary: impl FnMut(Boundary) -> Result<()>,
) -> Result<serde_json::Value> {
    let mut budget = Budget::new(Limits::default());
    let (snapshot, snapshot_bytes) = retirement::observation(
        &config,
        input.observation,
        input.observation_sha256,
        &mut budget,
    )?;
    let (plan, validated) = sealed::open_with_limits(
        &config,
        input.plan,
        input.plan_sha256,
        budget.remaining_limits()?,
    )?;
    drop(validated);
    let selection = retirement::analyze(
        &config,
        &snapshot,
        input.observation_sha256,
        &plan.inventory,
        &mut budget,
    )?;
    drop(snapshot);
    let output = Output::create_protected(&config, destination, Some(input.plan))?;
    output.bytes("observation.json", &snapshot_bytes)?;
    let database = output.database()?;
    let directory = Arc::new(database.0.try_clone().map_err(|_| invalid())?);
    let archive_id = uuid::Uuid::new_v4().to_string();
    let archive = Archive::create_anchored(
        directory,
        config.clone(),
        archive_id.clone(),
        super::stage::PAGES,
    )
    .await?;
    let plan = Arc::new(plan);
    let budget = Arc::new(Mutex::new(budget));
    let initialized = async {
        let directory_identity = database.identity()?;
        let file_identity = database.file_identity("ledger.sqlite3")?;
        let mut records = 0_u64;
        let mut bytes = 0_u64;
        for (key, mut indexes) in selection.candidates {
            boundary(Boundary::Copy)?;
            indexes.sort_by_key(|index| ordering(&plan.inventory, *index));
            let mut source_hash = Sha256::new();
            for index in &indexes {
                source(
                    &plan.inventory,
                    *index,
                    &mut *budget.lock().map_err(|_| invalid())?,
                )?
                .hash_into(&mut source_hash);
            }
            let proof = GroupProof {
                key,
                source_sha256: format!("{:x}", source_hash.finalize()),
                observation_sha256: input.observation_sha256.into(),
                records: u32::try_from(indexes.len()).map_err(|_| invalid())?,
            };
            let retained_plan = Arc::clone(&plan);
            let retained_budget = Arc::clone(&budget);
            let receipt = archive
                .prepare(proof, move |row| {
                    let index = *indexes.get(row as usize).ok_or_else(invalid)?;
                    source(
                        &retained_plan.inventory,
                        index,
                        &mut *retained_budget.lock().map_err(|_| invalid())?,
                    )
                })
                .await?;
            if receipt.complete {
                return Err(invalid());
            }
            records = records.checked_add(receipt.records).ok_or_else(invalid)?;
            bytes = bytes.checked_add(receipt.bytes).ok_or_else(invalid)?;
        }
        Ok((directory_identity, file_identity, records, bytes))
    }
    .await;
    let closed = archive.close().await;
    let (directory_identity, file_identity, records, bytes) = initialized?;
    closed?;
    boundary(Boundary::Closed)?;
    output.verify_database(&database)?;
    let mut budget = budget.lock().map_err(|_| invalid())?;
    for name in database.names(&mut budget)? {
        database.sync_file(&name)?;
    }
    database.0.sync_all().map_err(|_| invalid())?;
    let closed_directory = database.identity()?;
    let closed_file = database.file_identity("ledger.sqlite3")?;
    if (directory_identity.device, directory_identity.inode)
        != (closed_directory.device, closed_directory.inode)
        || (file_identity.device, file_identity.inode) != (closed_file.device, closed_file.inode)
    {
        return Err(changed());
    }
    let database_sha256 = database.hash_file(
        "ledger.sqlite3",
        u64::from(super::stage::PAGES) * 4096,
        &mut budget,
    )?;
    // Recheck the retained backup, never the live source. Its independent seal
    // is still required for later restore and maintenance planning.
    sealed::open_with_limits(
        &config,
        input.plan,
        input.plan_sha256,
        budget.remaining_limits()?,
    )?;
    boundary(Boundary::Ready)?;
    output.verify_database(&database)?;
    if database.identity()? != closed_directory
        || database.file_identity("ledger.sqlite3")? != closed_file
    {
        return Err(changed());
    }
    let mut summary = selection.summary;
    summary["format"] = "runtime-ledger-retirement-archive-v1".into();
    summary["admission"] = "retirement_held".into();
    summary["writer_exclusion"] = "unproved".into();
    summary["plan_sha256"] = input.plan_sha256.into();
    summary["database_sha256"] = database_sha256.into();
    summary["archived_records"] = records.into();
    summary["archived_bytes"] = bytes.into();
    summary["source_deactivated"] = false.into();
    summary["stale_replay_protection"] = "archive_not_activated".into();
    let ready_sha256 = output.json(
        "ready.json",
        &serde_json::json!({
            "format": "runtime-ledger-retirement-archive-ready-v1",
            "archive_id": archive_id,
            "summary": summary,
        }),
    )?;
    summary["ready_sha256"] = ready_sha256.into();
    Ok(summary)
}

fn ordering(inventory: &Inventory, index: usize) -> (u64, u8, &str) {
    let entry = &inventory.entries[index];
    let root = &inventory.roots[entry.directory];
    (root.scope, kind(root.kind), &entry.name)
}

fn kind(value: Kind) -> u8 {
    match value {
        Kind::Journal => 1,
        Kind::Authority => 2,
        _ => 0,
    }
}

fn source(inventory: &Inventory, index: usize, budget: &mut Budget) -> Result<SourceRecord> {
    let (scope, kind, name) = ordering(inventory, index);
    if kind == 0 || inventory.entries[index].temporary {
        return Err(invalid());
    }
    Ok(SourceRecord {
        scope,
        kind,
        name: name.into(),
        bytes: inventory.read(index, budget)?,
    })
}
