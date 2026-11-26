use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{hold, io, BARRIER, MANIFEST, PAGES, STAGE, TEMP};
use crate::config::RuntimeConfig;
use crate::failure::Result;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Descriptor {
    pub(super) version: u8,
    pub(super) archive: String,
    pub(super) sandbox: String,
    pub(super) home: String,
    pub(super) state_root: PathBuf,
    pub(super) home_root: PathBuf,
    pub(super) transfer_root: PathBuf,
    pub(super) born: u64,
    pub(super) pages: u32,
    // None binds the V1 format. Some pins the complete active V2 descriptor,
    // including its store UUID, so a replaced active store cannot be adopted.
    pub(super) active_v2_sha256: Option<String>,
}

impl Descriptor {
    pub(super) fn load(config: &RuntimeConfig) -> Result<Self> {
        let bytes = io::read(&config.state_root.join(MANIFEST))?;
        let descriptor = Self::decode(config, &bytes, active_format(config)?.as_deref())?;
        if io::read(&descriptor.marker())? != bytes {
            return Err(hold());
        }
        // A committed activation must have consumed its publication artifacts.
        // Their reappearance is a maintenance operation, never serving authority.
        for name in [STAGE, TEMP] {
            if io::exists(&config.state_root.join(name))? {
                return Err(hold());
            }
        }
        descriptor.inodes()?;
        Ok(descriptor)
    }

    pub(super) fn decode(
        config: &RuntimeConfig,
        bytes: &[u8],
        active_digest: Option<&str>,
    ) -> Result<Self> {
        let descriptor: Self = serde_json::from_slice(bytes).map_err(|_| hold())?;
        if !matches!(descriptor.version, 1..=3)
            || (descriptor.version == 3 && descriptor.active_v2_sha256.is_none())
            || descriptor.sandbox != config.sandbox_id
            || Some(descriptor.home.as_str()) != config.home_identity.as_deref()
            || descriptor.state_root != config.state_root
            || descriptor.home_root != config.home
            || descriptor.transfer_root != config.checkpoint_root
            || descriptor.born == 0
            || descriptor.born > config.generation
            || descriptor.pages != PAGES
            || uuid::Uuid::parse_str(&descriptor.archive)
                .map(|id| id.to_string())
                .ok()
                .as_deref()
                != Some(descriptor.archive.as_str())
            || descriptor.active_v2_sha256.as_deref() != active_digest
        {
            return Err(hold());
        }
        Ok(descriptor)
    }

    pub(super) fn marker(&self) -> PathBuf {
        self.transfer_root.join("runtime-state").join(BARRIER)
    }

    pub(super) fn inodes(&self) -> Result<[(u64, u64); 2]> {
        let mut identities = [(0, 0); 2];
        for (index, path) in [self.state_root.join(MANIFEST), self.marker()]
            .iter()
            .enumerate()
        {
            io::directories(path.parent().ok_or_else(hold)?)?;
            let metadata = std::fs::symlink_metadata(path).map_err(|_| hold())?;
            if !metadata.is_file()
                || metadata.nlink() != 1
                || metadata.mode() & 0o077 != 0
                || metadata.uid() != unsafe { libc::geteuid() }
            {
                return Err(hold());
            }
            identities[index] = (metadata.dev(), metadata.ino());
        }
        Ok(identities)
    }
}

pub(super) fn active_format(config: &RuntimeConfig) -> Result<Option<String>> {
    let path = config.state_root.join("mutation-ledger-v2.current.json");
    if io::exists(&path)? {
        return Ok(Some(format!("{:x}", Sha256::digest(io::read(&path)?))));
    }
    Ok(None)
}
