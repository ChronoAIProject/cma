use std::fs::File;
use std::os::fd::AsRawFd;

use super::super::inventory::Inventory;
use super::super::io::{Directory, Identity};
use super::super::{changed, Budget};
use super::error;
use crate::config::RuntimeConfig;
use crate::failure::Result;

pub(super) fn readonly(file: &File) -> Result<()> {
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: a live descriptor and writable statvfs storage are supplied.
    if unsafe { libc::fstatvfs(file.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(error("ledger_evidence_mount_unavailable"));
    }
    // SAFETY: successful fstatvfs initialized the structure.
    let stats = unsafe { stats.assume_init() };
    if stats.f_flag & libc::ST_RDONLY == 0 {
        return Err(error("ledger_evidence_readonly_required"));
    }
    Ok(())
}

pub(super) fn preflight(
    config: &RuntimeConfig,
    budget: &mut Budget,
    mount_check: fn(&File) -> Result<()>,
) -> Result<()> {
    for path in [&config.home, &config.state_root, &config.checkpoint_root] {
        budget.check()?;
        mount_check(&Directory::open(path)?.0)?;
    }
    let state = Directory::open(&config.state_root)?;
    for name in state.names(budget)? {
        if [
            "mutation-ledger-",
            ".mutation-ledger-",
            "mutation-retirement-",
            ".mutation-retirement-",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        {
            return Err(error("ledger_evidence_format_unsupported"));
        }
    }
    Ok(())
}

pub(super) fn verify(
    inventory: &Inventory,
    budget: &mut Budget,
    mount_check: fn(&File) -> Result<()>,
) -> Result<()> {
    for root in &inventory.roots {
        budget.check()?;
        if let Some(expected) = &root.identity {
            let directory = Directory::open(&root.path)?;
            mount_check(&directory.0)?;
            if directory.identity()? != *expected {
                return Err(changed());
            }
        }
    }
    for entry in &inventory.entries {
        budget.check()?;
        let root = &inventory.roots[entry.directory];
        let directory = Directory::open(&root.path)?;
        let file = directory.open_at(&entry.name, 0)?.ok_or_else(changed)?;
        mount_check(&file)?;
        if Identity::from(file.metadata().map_err(|_| changed())?) != entry.identity {
            return Err(changed());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_writable_mount_is_refused() {
        let root = std::env::temp_dir();
        assert_eq!(
            readonly(&File::open(root).unwrap()).unwrap_err().code,
            "ledger_evidence_readonly_required"
        );
    }
}
