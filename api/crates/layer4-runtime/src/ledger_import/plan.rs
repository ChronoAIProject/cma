//! A sealed byte backup is inspection evidence, never maintenance authority.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::inventory::{self, Inventory};
use super::plan_io::Archive;
use super::{changed, validation, Budget, Limits};
use crate::config::RuntimeConfig;
use crate::failure::Result;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Plan {
    pub(super) format: String,
    pub(super) admission: String,
    pub(super) writer_exclusion: String,
    pub(super) sandbox: String,
    pub(super) home_identity: Option<String>,
    pub(super) generation: u64,
    pub(super) home_root: PathBuf,
    pub(super) state_root: PathBuf,
    pub(super) transfer_root: PathBuf,
    pub(super) inventory: Inventory,
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
    let plan = Plan {
        format: "runtime-ledger-plan-v1".into(),
        admission: "import_held".into(),
        writer_exclusion: "unproved".into(),
        sandbox: config.sandbox_id.clone(),
        home_identity: config.home_identity.clone(),
        generation: config.generation,
        home_root: config.home.clone(),
        state_root: config.state_root.clone(),
        transfer_root: config.checkpoint_root.clone(),
        inventory,
    };
    let manifest = archive.json("plan.json", &plan)?;
    let inventory = &plan.inventory;
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
    let mut summary = analysis.summary(inventory);
    summary["format"] = "runtime-ledger-plan-v1".into();
    summary["plan_sha256"] = manifest.into();
    summary["backup"] = "sealed".into();
    Ok(summary)
}
