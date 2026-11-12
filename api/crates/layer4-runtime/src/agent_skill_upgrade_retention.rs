//! Bounded, non-evicting preservation outside skill discovery on the same home.
use super::{conflict, io};
use crate::failure::{Result, RuntimeError};
use std::path::Path;

const MAX_TREES: usize = 32;
const MAX_BYTES: u64 = 512 * 1024 * 1024;

fn full() -> RuntimeError {
    RuntimeError::rejected(
        "agent_skill_upgrade_retention_full",
        "Retained skill trees reached 32 entries or 512 MiB; archive and remove settled trees from the Agent home's .cma-skill-upgrades directory before a new attempt. Never remove pending operation evidence.",
    )
}

pub(super) fn prepare(stage: &Path) -> Result<()> {
    let parent = stage.parent().expect("retention parent");
    match std::fs::create_dir(parent) {
        Ok(()) => crate::fs_durability::sync_parent(parent).map_err(io)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(io(e)),
    }
    if !crate::paths::path_is_directory(parent) {
        return Err(conflict("skill retention parent is not a real directory"));
    }
    Ok(())
}

pub(super) fn reserve(stage: &Path, incoming: u64) -> Result<()> {
    let parent = stage.parent().expect("retention parent");
    let mut count = 0usize;
    // Include the installation receipt in each retained tree's size budget.
    let mut total = incoming.saturating_add(4096);
    for entry in std::fs::read_dir(parent).map_err(io)? {
        let path = entry.map_err(io)?.path();
        if path == stage {
            continue;
        }
        count += 1;
        if count >= MAX_TREES {
            return Err(full());
        }
        let mut entries = 0;
        measure(&path, &mut total, &mut entries, 0)?;
    }
    if total > MAX_BYTES {
        return Err(full());
    }
    Ok(())
}

fn measure(path: &Path, total: &mut u64, entries: &mut usize, depth: usize) -> Result<()> {
    *entries += 1;
    if *entries > 16384 || depth > 128 {
        return Err(full());
    }
    let metadata = std::fs::symlink_metadata(path).map_err(io)?;
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path).map_err(io)? {
            measure(&entry.map_err(io)?.path(), total, entries, depth + 1)?;
        }
    } else if metadata.is_file() && !metadata.is_symlink() {
        *total = total.saturating_add(metadata.len());
        if *total > MAX_BYTES {
            return Err(full());
        }
    } else {
        return Err(conflict(
            "retained skill tree contains a link or special entry",
        ));
    }
    Ok(())
}
