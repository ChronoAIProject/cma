//! Real allocated blocks, never a sparse logical-length promise. Once spent,
//! the reserve is not replenished automatically and new work remains held.
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::{diagnostics, invalid, paths, storage_error};
use crate::failure::Result;

pub(crate) const BYTES: u64 = 16 * 1024 * 1024;

pub(super) fn prepare(directory: &Path, create: bool) -> Result<()> {
    if create {
        let mut file = paths::open_file(&directory.join("emergency.reserve"), true)?;
        // Writes, unlike set_len(), allocate actual storage on supported filesystems.
        let block = [0xa5; 64 * 1024];
        for _ in 0..BYTES / block.len() as u64 {
            file.write_all(&block).map_err(|error| {
                diagnostics::io(
                    "storage.reserve.write",
                    "create_reserve",
                    "emergency_reserve",
                    &error,
                    storage_error,
                )
            })?;
        }
        file.sync_all().map_err(|error| {
            diagnostics::io(
                "storage.reserve.file_sync",
                "create_reserve",
                "emergency_reserve",
                &error,
                storage_error,
            )
        })?;
        crate::fs_durability::sync_directory(directory).map_err(|error| {
            diagnostics::io(
                "storage.reserve.directory_sync",
                "create_reserve",
                "store_directory",
                &error,
                storage_error,
            )
        })?;
    }
    remaining(directory)?;
    Ok(())
}

pub(crate) fn remaining(directory: &Path) -> Result<u64> {
    let file = paths::open_file(&directory.join("emergency.reserve"), false)?;
    let metadata = file.metadata().map_err(|error| {
        diagnostics::io(
            "storage.reserve.metadata",
            "inspect_reserve",
            "emergency_reserve",
            &error,
            storage_error,
        )
    })?;
    match metadata.len() {
        0 => Ok(0),
        BYTES
            if metadata
                .blocks()
                .checked_mul(512)
                .is_some_and(|bytes| bytes >= BYTES) =>
        {
            Ok(BYTES)
        }
        _ => {
            tracing::error!(
                predicate = "storage.reserve.allocation",
                operation = "inspect_reserve",
                object_role = "emergency_reserve",
                length = metadata.len(),
                blocks = metadata.blocks(),
                required_bytes = BYTES,
                "runtime ledger reserve allocation refused"
            );
            Err(invalid())
        }
    }
}

pub(crate) fn require(directory: &Path) -> Result<()> {
    if remaining(directory)? != BYTES {
        return Err(diagnostics::predicate(
            "storage.reserve.exhausted",
            "require_reserve",
            "emergency_reserve",
            storage_error,
        ));
    }
    Ok(())
}

pub(super) fn release(directory: &Path) -> Result<()> {
    let file = paths::open_file(&directory.join("emergency.reserve"), false)?;
    file.set_len(0).map_err(|error| {
        diagnostics::io(
            "storage.reserve.truncate",
            "release_reserve",
            "emergency_reserve",
            &error,
            storage_error,
        )
    })?;
    file.sync_all().map_err(|error| {
        diagnostics::io(
            "storage.reserve.file_sync",
            "release_reserve",
            "emergency_reserve",
            &error,
            storage_error,
        )
    })
}
