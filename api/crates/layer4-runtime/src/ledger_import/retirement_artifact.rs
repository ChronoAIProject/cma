//! Verify detached archive bytes against an independently retained ready hash.
//! This is a read-only artifact check, never a source-deactivation receipt.
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde::Deserialize;

use super::{changed, invalid, io::Directory, retirement, Budget, Limits};
use crate::config::RuntimeConfig;
use crate::failure::Result;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    format: String,
    archive_id: String,
    summary: Summary,
}

#[derive(Deserialize)]
struct Summary {
    format: String,
    admission: String,
    writer_exclusion: String,
    source_mutated: bool,
    source_deactivated: bool,
    apply_authority: bool,
    api_snapshot_sha256: String,
    plan_sha256: String,
    database_sha256: String,
    candidate_resource_groups: u64,
    candidate_journal_files: u64,
    candidate_authority_files: u64,
    archived_records: u64,
    archived_bytes: u64,
}

pub(super) fn verify(
    config: &RuntimeConfig,
    path: &Path,
    expected: &str,
) -> Result<serde_json::Value> {
    if !digest(expected) {
        return Err(invalid());
    }
    let directory = Directory::open(path)?;
    private(&directory)?;
    let original_directory = directory.identity()?;
    let mut budget = Budget::new(Limits::default());
    if directory.names(&mut budget)? != ["database", "observation.json", "ready.json"] {
        return Err(invalid());
    }
    if !directory.file_identity("ready.json")?.sealed_file()
        || !directory.file_identity("observation.json")?.sealed_file()
    {
        return Err(invalid());
    }
    let (ready, hash): (Ready, _) =
        directory.deserialize("ready.json", 24 * 1024 * 1024, &mut budget)?;
    if hash != expected
        || ready.format != "runtime-ledger-retirement-archive-ready-v1"
        || uuid::Uuid::parse_str(&ready.archive_id)
            .map(|id| id.to_string())
            .ok()
            .as_deref()
            != Some(ready.archive_id.as_str())
    {
        return Err(changed());
    }
    let summary = ready.summary;
    if summary.format != "runtime-ledger-retirement-archive-v1"
        || summary.admission != "retirement_held"
        || summary.writer_exclusion != "unproved"
        || summary.source_mutated
        || summary.source_deactivated
        || summary.apply_authority
        || !digest(&summary.plan_sha256)
        || !digest(&summary.database_sha256)
        || summary.archived_records > Limits::default().entries as u64
        || summary.archived_bytes > Limits::default().bytes
        || summary.candidate_resource_groups > summary.archived_records
        || summary
            .candidate_journal_files
            .checked_add(summary.candidate_authority_files)
            != Some(summary.archived_records)
    {
        return Err(invalid());
    }
    let (_, observation_bytes) = retirement::observation(
        config,
        &path.join("observation.json"),
        &summary.api_snapshot_sha256,
        &mut budget,
    )?;
    // Require the pinned observation through the original directory descriptor
    // too, so replacing the visible directory cannot mix two artifacts.
    let (_, pinned_bytes) = directory.read("observation.json", 4 * 1024 * 1024, &mut budget)?;
    if observation_bytes != pinned_bytes {
        return Err(changed());
    }
    let database = directory.child("database")?.ok_or_else(invalid)?;
    private(&database)?;
    let database_identity = database.identity()?;
    if database.names(&mut budget)? != ["emergency.reserve", "ledger.sqlite3", "writer.lock"]
        || database.file_identity("writer.lock")?.bytes != 0
        || database.file_identity("emergency.reserve")?.bytes
            != crate::ledger_storage::reserve::BYTES
        || database.hash_file(
            "ledger.sqlite3",
            u64::from(super::stage::PAGES) * 4096,
            &mut budget,
        )? != summary.database_sha256
    {
        return Err(changed());
    }
    if directory.identity()? != original_directory
        || Directory::open(path)?.identity()? != original_directory
        || directory
            .child("database")?
            .ok_or_else(changed)?
            .identity()?
            != database_identity
    {
        return Err(changed());
    }
    // Never echo arbitrary fields from a supplied artifact, even when pinned.
    Ok(serde_json::json!({
        "format": "runtime-ledger-retirement-archive-verification-v1",
        "ready_sha256": expected,
        "database_sha256": summary.database_sha256,
        "plan_sha256": summary.plan_sha256,
        "api_snapshot_sha256": summary.api_snapshot_sha256,
        "archived_records": summary.archived_records,
        "archived_bytes": summary.archived_bytes,
        "candidate_resource_groups": summary.candidate_resource_groups,
        "admission": "retirement_held",
        "source_deactivated": false,
        "apply_authority": false,
    }))
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn private(directory: &Directory) -> Result<()> {
    let metadata = directory.0.metadata().map_err(|_| invalid())?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(invalid());
    }
    Ok(())
}
