//! Reopen only an exact, independently pinned plan; never consult live sources.
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::io::Directory;
use super::plan::Plan;
use super::{changed, invalid, validation, Budget, Limits};
use crate::config::RuntimeConfig;
use crate::failure::Result;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Seal {
    format: String,
    admission: String,
    writer_exclusion: String,
    plan_sha256: String,
    inventory_sha256: String,
    files: usize,
}

pub(super) fn open(
    config: &RuntimeConfig,
    path: &Path,
    expected: &str,
) -> Result<(Plan, validation::Analysis)> {
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid());
    }
    let directory = Directory::open(path)?;
    let metadata = directory.0.metadata().map_err(|_| invalid())?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(invalid());
    }
    let identity = directory.identity()?;
    let mut budget = Budget::new(Limits::default());
    let (seal, _): (Seal, _) = directory.deserialize("sealed.json", 4096, &mut budget)?;
    if seal.format != "runtime-ledger-plan-seal-v1"
        || seal.admission != "import_held"
        || seal.writer_exclusion != "unproved"
        || seal.plan_sha256 != expected
        || seal.files > Limits::default().entries
    {
        return Err(invalid());
    }
    let (mut plan, hash): (Plan, _) =
        directory.deserialize("plan.json", 24 * 1024 * 1024, &mut budget)?;
    if hash != expected
        || plan.format != "runtime-ledger-plan-v1"
        || plan.admission != "import_held"
        || plan.writer_exclusion != "unproved"
        || plan.sandbox != config.sandbox_id
        || plan.home_identity != config.home_identity
        || plan.generation != config.generation
        || plan.home_root != config.home
        || plan.state_root != config.state_root
        || plan.transfer_root != config.checkpoint_root
        || plan.inventory.digest != seal.inventory_sha256
        || plan.inventory.entries.len() != seal.files
    {
        return Err(invalid());
    }
    // Count and validate every file, including otherwise ignored temporary
    // authority. Extra files, links and modifications cannot become a new plan.
    let names = directory.names(&mut budget)?;
    if names.len() != seal.files + 2 {
        return Err(invalid());
    }
    for (index, name) in names.iter().take(seal.files).enumerate() {
        if name != &format!("{index:08}.record") {
            return Err(invalid());
        }
    }
    if names[seal.files..] != ["plan.json", "sealed.json"] {
        return Err(invalid());
    }
    plan.inventory.archive = Some((path.into(), identity.clone()));
    let analysis = validation::analyze_budget(
        config,
        &plan.inventory,
        Budget::new(budget.remaining_limits()?),
    )?;
    if directory.identity()? != identity || Directory::open(path)?.identity()? != identity {
        return Err(changed());
    }
    Ok((plan, analysis))
}
