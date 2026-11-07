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
        let mut changed_directories = BTreeSet::new();
        for (index, file) in files.iter().enumerate() {
            if !keep.contains(&index) {
                std::fs::remove_file(&file.path)
                    .map_err(|error| io_error("prune redundant mutation authority", error))?;
                changed_directories.insert(file.path.parent().expect("validated authority path"));
            }
        }
        // A crash during deletion leaves only redundant valid predecessors.
        // Commit all deletions before another append can spend physical space.
        for directory in changed_directories {
            crate::fs_durability::sync_directory(directory)
                .map_err(|error| io_error("sync pruned mutation authority directory", error))?;
        }
    }
    Ok(files
        .into_iter()
        .enumerate()
        .filter_map(|(index, file)| keep.contains(&index).then_some(file))
        .collect())
}
