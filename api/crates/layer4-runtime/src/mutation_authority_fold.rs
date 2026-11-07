//! V1-compatible folding. Survivors become durable before any predecessor dies.
use std::collections::{BTreeMap, BTreeSet};

use super::{io, io_error, Disposition, ResourceKey};
use crate::failure::Result;

pub(super) fn prune(files: Vec<io::AuthorityFile>) -> Result<Vec<io::AuthorityFile>> {
    let mut grouped = BTreeMap::<ResourceKey, Vec<usize>>::new();
    for (index, file) in files.iter().enumerate() {
        grouped
            .entry(file.record.resource.clone())
            .or_default()
            .push(index);
    }
    let mut keep = BTreeSet::new();
    for indices in grouped.values() {
        let present = indices
            .iter()
            .copied()
            .filter(|&i| files[i].record.disposition == Disposition::Present);
        let absent = indices
            .iter()
            .copied()
            .filter(|&i| files[i].record.disposition == Disposition::Absent);
        keep.extend(present.max_by_key(|&i| files[i].record.authority_fence));
        keep.extend(
            absent
                .clone()
                .min_by_key(|&i| files[i].record.authority_fence),
        );
        keep.extend(absent.max_by_key(|&i| files[i].record.authority_fence));
    }
    if keep.len() != files.len() {
        // This also repairs durability after an ambiguous prior publish error.
        // Never trust mere visibility of the newest file as a sync barrier.
        for &index in &keep {
            io::sync_record(&files[index].path)?;
        }
        for (index, file) in files.iter().enumerate() {
            if !keep.contains(&index) {
                std::fs::remove_file(&file.path)
                    .map_err(|error| io_error("prune redundant mutation authority", error))?;
            }
        }
    }
    // Each populated root retains a survivor. Always sync its directory before
    // returning capacity, even on a no-op fold: a prior prune may have unlinked
    // predecessors but failed its final barrier, including before a restart.
    let directories = files
        .iter()
        .map(|file| file.path.parent().expect("validated authority path"))
        .collect::<BTreeSet<_>>();
    for directory in directories {
        sync_directory(directory).map_err(|error| {
            io_error("sync mutation authority directory before admission", error)
        })?;
    }
    Ok(files
        .into_iter()
        .enumerate()
        .filter_map(|(index, file)| keep.contains(&index).then_some(file))
        .collect())
}

// The fault seam is local to folding; unrelated filesystem operations retain
// their real durability path in tests as well as production.
fn sync_directory(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(test)]
    fault::before_sync(path)?;
    crate::fs_durability::sync_directory(path)?;
    #[cfg(test)]
    fault::after_sync();
    Ok(())
}

#[cfg(test)]
#[path = "mutation_authority_fold_fault.rs"]
pub(super) mod fault;
