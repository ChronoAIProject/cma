//! A sealed byte backup is inspection evidence, never maintenance authority.
use std::path::Path;

use serde::Serialize;

use super::inventory::{self, Inventory};
use super::plan_io::Archive;
use super::{changed, validation, Budget, Limits};
use crate::config::RuntimeConfig;
use crate::failure::Result;

#[derive(Serialize)]
struct Plan<'a> {
    format: &'static str,
    admission: &'static str,
    writer_exclusion: &'static str,
    sandbox: &'a str,
    home_identity: &'a Option<String>,
    generation: u64,
    home_root: &'a Path,
    state_root: &'a Path,
    transfer_root: &'a Path,
    inventory: &'a Inventory,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Boundary {
    Copy,
    Manifest,
    Seal,
}

pub(super) fn create(config: &RuntimeConfig, destination: &Path) -> Result<serde_json::Value> {
    create_checked(config, destination, |_| Ok(()))
}

pub(super) fn create_checked(
    config: &RuntimeConfig,
    destination: &Path,
    mut boundary: impl FnMut(Boundary) -> Result<()>,
) -> Result<serde_json::Value> {
    let mut budget = Budget::new(Limits::default());
    let inventory = inventory::scan(config, budget.remaining_limits()?)?;
    let analysis =
        validation::analyze_budget(config, &inventory, Budget::new(budget.remaining_limits()?))?;
    let archive = Archive::create(config, destination)?;
    for index in 0..inventory.entries.len() {
        boundary(Boundary::Copy)?;
        let bytes = inventory.read(index, &mut budget)?;
        archive.bytes(&format!("{index:08}.record"), &bytes)?;
    }
    boundary(Boundary::Manifest)?;
    let manifest = archive.json(
        "plan.json",
        &Plan {
            format: "runtime-ledger-plan-v1",
            admission: "import_held",
            writer_exclusion: "unproved",
            sandbox: &config.sandbox_id,
            home_identity: &config.home_identity,
            generation: config.generation,
            home_root: &config.home,
            state_root: &config.state_root,
            transfer_root: &config.checkpoint_root,
            inventory: &inventory,
        },
    )?;
    boundary(Boundary::Seal)?;
    // Re-enumerate to detect added/deleted entries, root replacement or drift
    // after an earlier file was copied. This is not old-writer exclusion.
    if inventory::scan(config, budget.remaining_limits()?)?.digest != inventory.digest {
        return Err(changed());
    }
    budget.check()?;
    archive.json(
        "sealed.json",
        &serde_json::json!({
            "format": "runtime-ledger-plan-seal-v1",
            "admission": "import_held", "writer_exclusion": "unproved",
            "plan_sha256": manifest, "inventory_sha256": inventory.digest,
            "files": inventory.entries.len(),
        }),
    )?;
    let mut summary = analysis.summary(&inventory);
    summary["format"] = "runtime-ledger-plan-v1".into();
    summary["plan_sha256"] = manifest.into();
    summary["backup"] = "sealed".into();
    Ok(summary)
}
