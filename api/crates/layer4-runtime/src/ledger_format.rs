//! Manifest is the only format commit point. Missing/partial V2 never means V1.
mod aborted;
mod descriptor;
#[cfg(test)]
mod diagnostics_tests;
pub(crate) mod io;
mod publication;
#[cfg(test)]
mod tests;
#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod v1_reader;
#[cfg(test)]
#[allow(dead_code)]
mod v1_validation;

use std::fs::File;
use std::sync::Arc;

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::mutation_ledger::Ledger;
use descriptor::Descriptor;

pub(crate) const BARRIER: &str = "mutation-ledger-v2.required.json";
const MANIFEST: &str = "mutation-ledger-v2.current.json";
const TEMP: &str = ".mutation-ledger-v2.current.tmp";
const LOCK: &str = "mutation-ledger-v2.lock";
const STAGE: &str = ".mutation-ledger-v2.preparing";
const DATABASE: &str = "mutation-ledger-v2";
// One GiB logical/page ceiling for new retained homes, plus the physical reserve.
// This is not an import budget or a claim about any exhausted legacy home.
const PAGES: u32 = 262_144;

pub(crate) struct Selection {
    fresh: bool,
    pending_publication: bool,
    descriptor: Option<Descriptor>,
    writer: Option<Arc<File>>,
}

pub(crate) struct Guard {
    descriptor: Descriptor,
    config: RuntimeConfig,
    pub(crate) writer: Arc<File>,
    inodes: [(u64, u64); 3],
}

impl Guard {
    fn new(descriptor: Descriptor, config: RuntimeConfig, writer: Arc<File>) -> Result<Self> {
        let inodes = inodes(&config)?;
        use std::os::unix::fs::MetadataExt;
        let metadata = writer
            .metadata()
            .map_err(|error| hold_io("guard.writer_metadata", "inspect_writer_metadata", error))?;
        if inodes[2] != (metadata.dev(), metadata.ino()) {
            return Err(hold_at("guard.writer_inode"));
        }
        Ok(Self {
            descriptor,
            config,
            writer,
            inodes,
        })
    }
    pub(crate) fn verify(&self) -> Result<()> {
        if inodes(&self.config)? != self.inodes
            || Descriptor::load(&self.config)? != self.descriptor
        {
            return Err(hold_at("guard.identity"));
        }
        self.descriptor.verify(self.descriptor.base_markers())?;
        self.descriptor
            .verify(self.descriptor.markers(self.config.generation))?;
        crate::persistent_home::verify(&self.config)?;
        Ok(())
    }
    pub(crate) fn opening(&self, store: &[u8], previous: u64) -> Result<()> {
        if uuid::Uuid::from_slice(store)
            .map(|id| id.to_string())
            .ok()
            .as_deref()
            != Some(&self.descriptor.store)
            || previous < self.descriptor.born
            || previous > self.config.generation
        {
            return Err(hold_at("guard.store_or_generation"));
        }
        self.descriptor.verify(self.descriptor.base_markers())?;
        if previous < self.config.generation {
            let root = self
                .config
                .state_root
                .join(format!("process-generation-{}", self.config.generation));
            // A half-published new scope holds. Never repair a missing barrier
            // in an already used scope by treating the scope as fresh.
            if io::exists(&root)? {
                return Err(hold_at("guard.existing_generation"));
            }
            self.descriptor
                .publish(self.descriptor.markers(self.config.generation))?;
        }
        self.verify()
    }
}

/// Called under the construction writer lock, before home initialization.
pub(crate) fn select(config: &RuntimeConfig) -> Result<Selection> {
    if io::exists(&config.state_root.join(publication::INTENT))? {
        retained_transfer(config)?;
        let writer = Arc::new(io::lock(&config.state_root.join(LOCK), false)?);
        let descriptor = publication::inspect(config)?;
        return Ok(Selection {
            fresh: false,
            pending_publication: true,
            descriptor: Some(descriptor),
            writer: Some(writer),
        });
    }
    let current = config.state_root.join(MANIFEST);
    if io::exists(&current)? {
        retained_transfer(config)?;
        let writer = Arc::new(io::lock(&config.state_root.join(LOCK), false)?);
        let descriptor = Descriptor::load(config)?;
        descriptor.verify(descriptor.base_markers())?;
        return Ok(Selection {
            fresh: false,
            pending_publication: false,
            descriptor: Some(descriptor),
            writer: Some(writer),
        });
    }
    if io::exists(&config.state_root.join(LOCK))? {
        retained_transfer(config)?;
        let writer = Arc::new(io::lock(&config.state_root.join(LOCK), false)?);
        aborted::discard(config)?;
        return Ok(Selection {
            fresh: true,
            pending_publication: false,
            descriptor: None,
            writer: Some(writer),
        });
    }
    for name in [STAGE, DATABASE, TEMP] {
        if io::exists(&config.state_root.join(name))? {
            return Err(hold_at("selection.partial_v2"));
        }
    }
    for path in [
        config.checkpoint_root.join("runtime-state").join(BARRIER),
        config.state_root.join("mutation-authority").join(BARRIER),
        config.state_root.join("runtime-mutations").join(BARRIER),
    ] {
        if io::exists(&path)? {
            return Err(hold_at("selection.required_barrier"));
        }
    }
    // Legacy/no-home and previously initialized homes retain V1. An empty G+1
    // process root does not establish freshness of the retained home.
    let mut fresh = config.home_identity.is_some()
        && !io::exists(&config.state_root.join("home-binding"))?
        && io::empty(&config.checkpoint_root.join("runtime-state"))?;
    if fresh {
        io::directories(&config.state_root)?;
        for entry in std::fs::read_dir(&config.state_root)
            .map_err(|error| hold_io("selection.state_inventory", "read_state_directory", error))?
        {
            let entry = entry
                .map_err(|error| hold_io("selection.state_entry", "read_state_entry", error))?;
            if entry.file_name() != "workspace-removal-writer-v1.lock"
                || !io::read(&entry.path())?.is_empty()
            {
                fresh = false;
                break;
            }
        }
        if fresh {
            retained_transfer(config)?;
        }
    }
    Ok(Selection {
        fresh,
        pending_publication: false,
        descriptor: None,
        writer: None,
    })
}

impl Selection {
    /// Called after home initialization, still before installation/effects.
    pub(crate) async fn open(self, config: &RuntimeConfig) -> Result<Option<Arc<Ledger>>> {
        let (descriptor, writer) = if let Some(descriptor) = self.descriptor {
            if self.pending_publication {
                publication::resume(config, &descriptor)?;
            }
            (
                descriptor,
                self.writer
                    .ok_or_else(|| hold_at("selection.writer_absent"))?,
            )
        } else if self.fresh {
            let writer = match self.writer {
                Some(writer) => writer,
                None => Arc::new(io::lock(&config.state_root.join(LOCK), true)?),
            };
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "locked")?;
            let stage = config.state_root.join(STAGE);
            let ledger = Ledger::open(stage.clone(), config.clone(), true, PAGES).await?;
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "database")?;
            let store = ledger.store_id().await?;
            ledger.close().await?;
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "closed")?;
            crate::fs_durability::sync_tree(&stage)
                .map_err(|error| hold_io("publication.stage_sync", "sync_stage_tree", error))?;
            crate::publication_qualification::stop_before_intent()?;
            let descriptor = Descriptor::new(config, store)?;
            io::create(
                &config.state_root.join(publication::INTENT),
                &descriptor.bytes()?,
            )?;
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "intent")?;
            descriptor.publish(descriptor.base_markers())?;
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "barriers")?;
            let database = config.state_root.join(DATABASE);
            if io::exists(&database)? {
                return Err(hold_at("publication.database_exists"));
            }
            crate::fs_durability::rename(&stage, &database).map_err(|error| {
                hold_io(
                    "publication.database_rename",
                    "rename_database_and_sync_parents",
                    error,
                )
            })?;
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "renamed")?;
            io::create(&config.state_root.join(TEMP), &descriptor.bytes()?)?;
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "temporary")?;
            crate::fs_durability::rename(
                &config.state_root.join(TEMP),
                &config.state_root.join(MANIFEST),
            )
            .map_err(|error| {
                hold_io(
                    "publication.manifest_rename",
                    "rename_manifest_and_sync_parents",
                    error,
                )
            })?;
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "committed")?;
            publication::consume(config, &descriptor)?;
            (descriptor, writer)
        } else {
            return Ok(None);
        };
        let guard = Arc::new(Guard::new(descriptor, config.clone(), writer)?);
        let ledger = Ledger::open_guarded(
            config.state_root.join(DATABASE),
            config.clone(),
            PAGES,
            guard,
        )
        .await?;
        Ok(Some(Arc::new(ledger)))
    }
}

// Exact recognition only: no general exception to the installation scanner.
pub(crate) fn installation_marker(config: &RuntimeConfig) -> Result<()> {
    let descriptor = Descriptor::load(config)?;
    descriptor.verify(descriptor.base_markers())?;
    descriptor.verify(descriptor.markers(config.generation))
}

fn retained_transfer(config: &RuntimeConfig) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let transfer = config
        .process_env
        .get("TMPDIR")
        .ok_or_else(|| hold_at("transfer.tmpdir_missing"))?;
    if config.home_identity.is_none()
        || std::path::Path::new(transfer).join("cma-checkpoints") != config.checkpoint_root
    {
        return Err(hold_at("transfer.assignment_roots"));
    }
    // Kata can expose each subpath of one retained claim as a distinct
    // virtio-fs device. Device equality is not proof of retained-root identity.
    let mut device = None;
    let mut different_devices = false;
    for path in [
        &config.home,
        &config.state_root,
        std::path::Path::new(transfer),
    ] {
        io::directories(path)?;
        let dev = std::fs::symlink_metadata(path)
            .map_err(|error| {
                hold_io(
                    "transfer.root_metadata",
                    "inspect_assigned_root_metadata",
                    error,
                )
            })?
            .dev();
        different_devices |= device.is_some_and(|expected| expected != dev);
        device = Some(dev);
    }
    if different_devices {
        tracing::warn!(predicate = "transfer.same_device");
    }
    Ok(())
}

fn hold() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_format_hold",
        "runtime mutation ledger format requires verification",
    )
}

// Predicate names are fixed source literals, logged separately because legacy
// adapters recognize the exact wire code/detail pair. Never log paths,
// identities, metadata contents, or the operating system error's payload.
fn hold_at(predicate: &'static str) -> RuntimeError {
    tracing::error!(
        predicate,
        "runtime mutation ledger format requires verification"
    );
    hold()
}

// Preserve the actual failing syscall category without formatting its payload.
fn hold_io(
    predicate: &'static str,
    operation: &'static str,
    error: std::io::Error,
) -> RuntimeError {
    tracing::error!(predicate, operation, object_role = "ledger_format", error_kind = ?error.kind(), raw_errno = error.raw_os_error(),
        "runtime mutation ledger format requires verification");
    hold()
}

fn inodes(config: &RuntimeConfig) -> Result<[(u64, u64); 3]> {
    use std::os::unix::fs::MetadataExt;
    let paths = [
        config.state_root.join(DATABASE),
        config.state_root.join(DATABASE).join("ledger.sqlite3"),
        config.state_root.join(LOCK),
    ];
    let mut result = [(0, 0); 3];
    for (index, path) in paths.iter().enumerate() {
        io::directories(path.parent().ok_or_else(hold)?)?;
        let metadata = std::fs::symlink_metadata(path).map_err(|error| {
            hold_io(
                "guard.path_metadata",
                "inspect_guarded_path_metadata",
                error,
            )
        })?;
        if (index == 0 && !metadata.is_dir())
            || (index != 0 && (!metadata.is_file() || metadata.nlink() != 1))
        {
            return Err(hold_at("guard.path_type_or_links"));
        }
        result[index] = (metadata.dev(), metadata.ino());
    }
    Ok(result)
}

/// Decode captured metadata without selecting, locking or opening a store.
pub(crate) fn inspect_descriptor(config: &RuntimeConfig, bytes: &[u8]) -> Result<(u64, Vec<u8>)> {
    let descriptor = Descriptor::decode(config, bytes)?;
    Ok((descriptor.born, descriptor.bytes()?))
}
