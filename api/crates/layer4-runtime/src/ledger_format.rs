//! Manifest is the only format commit point. Missing/partial V2 never means V1.
mod descriptor;
pub(crate) mod io;
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
        let metadata = writer.metadata().map_err(|_| hold())?;
        if inodes[2] != (metadata.dev(), metadata.ino()) {
            return Err(hold());
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
            return Err(hold());
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
            return Err(hold());
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
                return Err(hold());
            }
            self.descriptor
                .publish(self.descriptor.markers(self.config.generation))?;
        }
        self.verify()
    }
}

/// Called under the construction writer lock, before home initialization.
pub(crate) fn select(config: &RuntimeConfig) -> Result<Selection> {
    let current = config.state_root.join(MANIFEST);
    if io::exists(&current)? {
        retained_transfer(config)?;
        let writer = Arc::new(io::lock(&config.state_root.join(LOCK), false)?);
        let descriptor = Descriptor::load(config)?;
        descriptor.verify(descriptor.base_markers())?;
        return Ok(Selection {
            fresh: false,
            descriptor: Some(descriptor),
            writer: Some(writer),
        });
    }
    for name in [LOCK, STAGE, DATABASE, TEMP] {
        if io::exists(&config.state_root.join(name))? {
            return Err(hold());
        }
    }
    for path in [
        config.checkpoint_root.join("runtime-state").join(BARRIER),
        config.state_root.join("mutation-authority").join(BARRIER),
        config.state_root.join("runtime-mutations").join(BARRIER),
    ] {
        if io::exists(&path)? {
            return Err(hold());
        }
    }
    // Legacy/no-home and previously initialized homes retain V1. An empty G+1
    // process root does not establish freshness of the retained home.
    let mut fresh = config.home_identity.is_some()
        && !io::exists(&config.state_root.join("home-binding"))?
        && io::empty(&config.checkpoint_root.join("runtime-state"))?;
    if fresh {
        io::directories(&config.state_root)?;
        for entry in std::fs::read_dir(&config.state_root).map_err(|_| hold())? {
            let entry = entry.map_err(|_| hold())?;
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
        descriptor: None,
        writer: None,
    })
}

impl Selection {
    /// Called after home initialization, still before installation/effects.
    pub(crate) async fn open(self, config: &RuntimeConfig) -> Result<Option<Arc<Ledger>>> {
        let (descriptor, writer) = if let Some(descriptor) = self.descriptor {
            (descriptor, self.writer.ok_or_else(hold)?)
        } else if self.fresh {
            let writer = Arc::new(io::lock(&config.state_root.join(LOCK), true)?);
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
            crate::fs_durability::sync_tree(&stage).map_err(|_| hold())?;
            let descriptor = Descriptor::new(config, store)?;
            descriptor.publish(descriptor.base_markers())?;
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "barriers")?;
            let database = config.state_root.join(DATABASE);
            if io::exists(&database)? {
                return Err(hold());
            }
            crate::fs_durability::rename(&stage, &database).map_err(|_| hold())?;
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "renamed")?;
            io::create(&config.state_root.join(TEMP), &descriptor.bytes()?)?;
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "temporary")?;
            crate::fs_durability::rename(
                &config.state_root.join(TEMP),
                &config.state_root.join(MANIFEST),
            )
            .map_err(|_| hold())?;
            #[cfg(test)]
            tests::checkpoint(&config.state_root, "committed")?;
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
    let transfer = config.process_env.get("TMPDIR").ok_or_else(hold)?;
    if config.home_identity.is_none()
        || std::path::Path::new(transfer).join("cma-checkpoints") != config.checkpoint_root
    {
        return Err(hold());
    }
    // The supported provider mounts these three subpaths from one retained
    // claim. Assigned TMPDIR and same-device roots exclude legacy /tmp layouts;
    // this local check is not a replacement for provider mount qualification.
    let mut device = None;
    for path in [
        &config.home,
        &config.state_root,
        std::path::Path::new(transfer),
    ] {
        io::directories(path)?;
        let dev = std::fs::symlink_metadata(path).map_err(|_| hold())?.dev();
        if device.is_some_and(|expected| expected != dev) {
            return Err(hold());
        }
        device = Some(dev);
    }
    Ok(())
}

fn hold() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_format_hold",
        "runtime mutation ledger format requires verification",
    )
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
        let metadata = std::fs::symlink_metadata(path).map_err(|_| hold())?;
        if (index == 0 && !metadata.is_dir())
            || (index != 0 && (!metadata.is_file() || metadata.nlink() != 1))
        {
            return Err(hold());
        }
        result[index] = (metadata.dev(), metadata.ino());
    }
    Ok(result)
}
