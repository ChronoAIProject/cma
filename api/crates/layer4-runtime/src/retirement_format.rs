//! Read an already activated retirement dependency. This module never creates
//! activation metadata or treats a partial publication as an unretired home.
mod descriptor;
#[cfg(test)]
mod tests;

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::ledger_format::io;
use crate::mutation_journal::Operation;
use crate::retirement_archive::{Archive, Version};
use descriptor::Descriptor;

pub(crate) const BARRIER: &str = "mutation-retirement-v1.required.json";
const MANIFEST: &str = "mutation-retirement-v1.current.json";
const DATABASE: &str = "mutation-retirement-v1";
const STAGE: &str = ".mutation-retirement-v1.preparing";
const TEMP: &str = ".mutation-retirement-v1.current.tmp";
const PAGES: u32 = 262_144;

pub(crate) struct Selection(Option<(Descriptor, [(u64, u64); 2])>);

pub(crate) struct Reader {
    archive: Archive,
    config: RuntimeConfig,
    descriptor: Descriptor,
    inodes: [(u64, u64); 2],
}

/// Check before persistent-home initialization, installation recovery or effects.
pub(crate) fn select(config: &RuntimeConfig) -> Result<Selection> {
    if !io::exists(&config.state_root.join(MANIFEST))? {
        require_absent(config)?;
        return Ok(Selection(None));
    }
    let descriptor = Descriptor::load(config)?;
    let inodes = descriptor.inodes()?;
    Ok(Selection(Some((descriptor, inodes))))
}

impl Selection {
    pub(crate) async fn open(self, config: &RuntimeConfig) -> Result<Option<Reader>> {
        let Some((descriptor, inodes)) = self.0 else {
            require_absent(config)?;
            return Ok(None);
        };
        let archive = Archive::open_version(
            config.state_root.join(DATABASE),
            config.clone(),
            descriptor.archive.clone(),
            false,
            descriptor.pages,
            match descriptor.version {
                1 => Version::Legacy,
                2 => Version::Indexed,
                3 => Version::Collection,
                _ => return Err(hold()),
            },
        )
        .await?;
        let reader = Reader {
            archive,
            config: config.clone(),
            descriptor,
            inodes,
        };
        let result = async {
            reader.verify()?;
            reader.archive.require_ready().await?;
            reader.verify()
        }
        .await;
        if let Err(original) = result {
            reader.archive.close().await?;
            return Err(original);
        }
        Ok(Some(reader))
    }
}

impl Reader {
    #[cfg(test)]
    pub(crate) async fn close(self) -> Result<()> {
        self.archive.close().await
    }

    pub(crate) fn verify(&self) -> Result<()> {
        if Descriptor::load(&self.config)? != self.descriptor
            || self.descriptor.inodes()? != self.inodes
        {
            return Err(hold());
        }
        self.archive.verify()
    }

    pub(crate) async fn authorize(&self, operations: &[Operation]) -> Result<()> {
        self.require_ready().await?;
        self.archive.authorize(operations).await?;
        self.verify()
    }

    pub(crate) async fn require_ready(&self) -> Result<()> {
        self.verify()?;
        self.archive.require_ready().await?;
        self.verify()
    }
}

/// An unactivated process must stop admitting if a retirement publication
/// appears underneath it. It must never select a new archive while serving.
pub(crate) fn require_absent(config: &RuntimeConfig) -> Result<()> {
    for path in [
        config.state_root.join(MANIFEST),
        config.state_root.join(DATABASE),
        config.state_root.join(STAGE),
        config.state_root.join(TEMP),
        config.checkpoint_root.join("runtime-state").join(BARRIER),
    ] {
        if io::exists(&path)? {
            return Err(hold());
        }
    }
    Ok(())
}

// Exact recognition only. The serving startup also opens the required archive
// and refuses unfinished deactivation before invoking installation recovery.
pub(crate) fn installation_marker(config: &RuntimeConfig) -> Result<()> {
    Descriptor::load(config).map(|_| ())
}

fn hold() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_retirement_format_hold",
        "runtime retirement dependency requires verification",
    )
}

/// Validate captured descriptor authority; database readiness is not assessed.
pub(crate) fn inspect_descriptor(
    config: &RuntimeConfig,
    bytes: &[u8],
    active_digest: Option<&str>,
) -> Result<u8> {
    Descriptor::decode(config, bytes, active_digest).map(|descriptor| descriptor.version)
}
