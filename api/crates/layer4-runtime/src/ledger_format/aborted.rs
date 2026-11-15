//! Discard only an initial store which never reached its published database path.
//! A missing manifest over a final database is ambiguous and remains held.
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use super::{
    descriptor::Descriptor, hold_at, io, publication, BARRIER, DATABASE, LOCK, MANIFEST, STAGE,
    TEMP,
};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::mutation_ledger::Ledger;

#[derive(PartialEq, Eq)]
struct Stamp {
    device: u64,
    inode: u64,
    mode: u32,
    size: u64,
}

type Inventory = BTreeMap<PathBuf, Stamp>;

fn entries(path: &Path, allowed: &[&str], inventory: &mut Inventory) -> Result<()> {
    if !io::exists(path)? {
        return Ok(());
    }
    io::directories(path)?;
    stamp(path, inventory)?;
    for entry in std::fs::read_dir(path)
        .map_err(|error| io_failure("read_directory", "recovery.inventory", error))?
    {
        let entry =
            entry.map_err(|error| io_failure("read_directory", "recovery.inventory", error))?;
        if !entry
            .file_name()
            .to_str()
            .is_some_and(|name| allowed.contains(&name))
        {
            return Err(hold_at("recovery.unknown_state"));
        }
        stamp(&entry.path(), inventory)?;
    }
    Ok(())
}

fn stamp(path: &Path, inventory: &mut Inventory) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| io_failure("inspect_metadata", "recovery.metadata", error))?;
    if !(metadata.is_dir() || (metadata.is_file() && metadata.nlink() == 1))
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(metadata_failure("recovery.path_shape", &metadata));
    }
    inventory.insert(
        path.to_owned(),
        Stamp {
            device: metadata.dev(),
            inode: metadata.ino(),
            mode: metadata.mode(),
            size: if metadata.is_dir() { 0 } else { metadata.len() },
        },
    );
    Ok(())
}

fn inventory(config: &RuntimeConfig) -> Result<Inventory> {
    // A transport acknowledgement is not the serving commit. Final DB, manifest,
    // durable intent, installation, retirement and unknown authority all exclude
    // this path, even when the database happens to have no application rows.
    for name in [publication::INTENT, MANIFEST, DATABASE, TEMP] {
        if io::exists(&config.state_root.join(name))? {
            return Err(hold_at("recovery.published_or_intent_state"));
        }
    }
    crate::persistent_home::verify_initial(config)?;
    crate::retirement_format::require_absent(config)?;
    super::retained_transfer(config)?;
    let mut result = Inventory::new();
    entries(
        &config.state_root,
        &[
            "home-binding",
            "workspace-removal-writer-v1.lock",
            "workspace-removal-writer-v1.json",
            LOCK,
            STAGE,
            "mutation-authority",
            "runtime-mutations",
            "process-generation-1",
        ],
        &mut result,
    )?;
    entries(
        &config.state_root.join("home-binding"),
        &["binding.json"],
        &mut result,
    )?;
    entries(
        &config.state_root.join("process-generation-1"),
        &["mutation-authority", "runtime-mutations"],
        &mut result,
    )?;
    entries(&config.checkpoint_root, &["runtime-state"], &mut result)?;
    for path in marker_paths(config) {
        entries(
            path.parent()
                .ok_or_else(|| hold_at("recovery.marker_parent"))?,
            &[BARRIER],
            &mut result,
        )?;
    }
    entries(
        &config.state_root.join(STAGE),
        &["writer.lock", "ledger.sqlite3", "emergency.reserve"],
        &mut result,
    )?;
    Ok(result)
}

fn marker_paths(config: &RuntimeConfig) -> Vec<PathBuf> {
    let mut paths = vec![config.checkpoint_root.join("runtime-state").join(BARRIER)];
    for root in [
        config.state_root.clone(),
        config.state_root.join("process-generation-1"),
    ] {
        for name in ["mutation-authority", "runtime-mutations"] {
            paths.push(root.join(name).join(BARRIER));
        }
    }
    paths
}

fn private_file(path: &Path, bound: u64) -> Result<File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| io_failure("read_existing", "recovery.file_open", error))?;
    let m = file
        .metadata()
        .map_err(|error| io_failure("inspect_metadata", "recovery.metadata", error))?;
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
        || m.len() > bound
    {
        return Err(metadata_failure("recovery.private_file", &m));
    }
    Ok(file)
}

fn incomplete(config: &RuntimeConfig) -> Result<Option<File>> {
    let stage = config.state_root.join(STAGE);
    if !io::exists(&stage)? {
        return Ok(None);
    }
    let m = std::fs::symlink_metadata(&stage)
        .map_err(|error| io_failure("inspect_metadata", "recovery.metadata", error))?;
    if !m.is_dir() || m.mode() & 0o077 != 0 {
        return Err(metadata_failure("recovery.stage_mode", &m));
    }
    let writer = stage.join("writer.lock");
    let lock = if io::exists(&writer)? {
        let file = private_file(&writer, 0)?;
        file.try_lock()
            .map_err(|_| hold_at("recovery.writer_held"))?;
        Some(file)
    } else {
        None
    };
    let database = stage.join("ledger.sqlite3");
    if io::exists(&database)? {
        private_file(&database, 0)?;
    }
    let reserve = stage.join("emergency.reserve");
    if io::exists(&reserve)? {
        let mut file = private_file(&reserve, crate::ledger_storage::reserve::BYTES)?
            .take(crate::ledger_storage::reserve::BYTES + 1);
        let mut total = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let size = file
                .read(&mut buffer)
                .map_err(|error| io_failure("read_reserve", "recovery.reserve_read", error))?;
            if size == 0 {
                break;
            }
            total += size as u64;
            if total > crate::ledger_storage::reserve::BYTES
                || buffer[..size].iter().any(|value| *value != 0xa5)
            {
                return Err(hold_at("recovery.reserve_content"));
            }
        }
    }
    Ok(lock)
}

/// Caller holds both the supervisor construction lock and the existing format lock.
pub(super) fn discard(config: &RuntimeConfig) -> Result<()> {
    let mut snapshot = inventory(config)?;
    let stage = config.state_root.join(STAGE);
    let database = stage.join("ledger.sqlite3");
    let nonempty = snapshot.get(&database).is_some_and(|stamp| stamp.size != 0);
    let mut descriptor = None;
    let proof = if nonempty {
        Some(
            crate::ledger_storage::inspect_closed(&stage, |db| {
                let store: Vec<u8> = db
                    .query_row(
                        "SELECT store_id FROM metadata WHERE singleton=1",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|_| hold_at("recovery.store_identity"))?;
                let store = uuid::Uuid::from_slice(&store)
                    .map_err(|_| hold_at("recovery.store_identity"))?
                    .to_string();
                Ledger::verify_fresh_publication(db, config, &store)?;
                descriptor = Some(Descriptor::new(config, store)?);
                Ok(())
            })
            .map_err(|_| hold_at("recovery.nonfresh_database"))?,
        )
    } else {
        None
    };
    let _incomplete_writer = if nonempty { None } else { incomplete(config)? };
    for path in marker_paths(config) {
        if io::exists(&path)? {
            let expected = descriptor
                .as_ref()
                .ok_or_else(|| hold_at("recovery.barrier_without_store"))?;
            expected.verify([path])?;
        }
    }
    if let Some(proof) = &proof {
        proof.verify_at(&stage)?;
    }
    #[cfg(test)]
    super::tests::replace_aborted_path(config);
    // Delete barriers before the unused DB. After DB deletion every remaining
    // file is a recognizable empty creation prefix, so another interruption can
    // restart this same bounded cleanup without an absence-based DB proof.
    let paths = marker_paths(config).into_iter().chain([
        database,
        stage.join("emergency.reserve"),
        stage.join("writer.lock"),
    ]);
    for (index, path) in paths.enumerate() {
        let _ = index;
        if snapshot.contains_key(&path) {
            if inventory(config)? != snapshot {
                return Err(hold_at("recovery.path_replaced"));
            }
            crate::fs_durability::remove_file(&path).map_err(|error| {
                io_failure("remove_file_and_sync", "recovery.remove_file", error)
            })?;
            snapshot.remove(&path);
            #[cfg(test)]
            super::tests::checkpoint(&config.state_root, &format!("discard-{index}"))?;
        }
    }
    if snapshot.contains_key(&stage) {
        if inventory(config)? != snapshot {
            return Err(hold_at("recovery.path_replaced"));
        }
        crate::fs_durability::remove_dir(&stage).map_err(|error| {
            io_failure("remove_directory_and_sync", "recovery.remove_stage", error)
        })?;
        #[cfg(test)]
        super::tests::checkpoint(&config.state_root, "discard-stage")?;
    }
    // Keep the stable outer lock inode held until the replacement is open.
    tracing::warn!(
        predicate = "recovery.unpublished_initial_store",
        "discarded unused initial ledger publication"
    );
    Ok(())
}

fn io_failure(
    operation: &'static str,
    predicate: &'static str,
    error: std::io::Error,
) -> crate::failure::RuntimeError {
    tracing::error!(predicate, operation, object_role = "initial_publication", error_kind = ?error.kind(), raw_errno = error.raw_os_error(),
        "runtime mutation ledger format requires verification");
    super::hold()
}

fn metadata_failure(
    predicate: &'static str,
    metadata: &std::fs::Metadata,
) -> crate::failure::RuntimeError {
    tracing::error!(
        predicate,
        operation = "inspect_existing",
        object_role = "initial_publication",
        observed_mode = metadata.mode() & 0o7777,
        required_group_other_mask = 0u32,
        regular_file = metadata.is_file(),
        directory = metadata.is_dir(),
        single_link = metadata.nlink() == 1,
        effective_owner = metadata.uid() == unsafe { libc::geteuid() },
        private_mode = metadata.mode() & 0o077 == 0,
        "runtime mutation ledger format requires verification"
    );
    super::hold()
}
