use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{hold, io, BARRIER, MANIFEST};
use crate::config::RuntimeConfig;
use crate::failure::Result;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Descriptor {
    pub(super) version: u8,
    pub(super) store: String,
    pub(super) sandbox: String,
    pub(super) home: String,
    pub(super) state_root: PathBuf,
    pub(super) home_root: PathBuf,
    pub(super) transfer_root: PathBuf,
    pub(super) born: u64,
    pub(super) admission: String,
    pub(super) pages: u32,
}

impl Descriptor {
    pub(super) fn new(config: &RuntimeConfig, store: String) -> Result<Self> {
        Ok(Self {
            version: 2,
            store,
            sandbox: config.sandbox_id.clone(),
            home: config.home_identity.clone().ok_or_else(hold)?,
            state_root: config.state_root.clone(),
            home_root: config.home.clone(),
            transfer_root: config.checkpoint_root.clone(),
            born: config.generation,
            admission: "fresh".into(),
            pages: super::PAGES,
        })
    }
    pub(super) fn load(config: &RuntimeConfig) -> Result<Self> {
        let descriptor: Self =
            serde_json::from_slice(&io::read(&config.state_root.join(MANIFEST))?)
                .map_err(|_| hold())?;
        let mut expected = Self::new(config, descriptor.store.clone())?;
        expected.born = descriptor.born;
        if descriptor != expected
            || descriptor.born == 0
            || descriptor.born > config.generation
            || uuid::Uuid::parse_str(&descriptor.store).is_err()
            || uuid::Uuid::parse_str(&descriptor.store)
                .map(|id| id.to_string())
                .ok()
                .as_deref()
                != Some(&descriptor.store)
        {
            return Err(hold());
        }
        Ok(descriptor)
    }
    pub(super) fn bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|_| hold())
    }
    pub(super) fn install(&self) -> PathBuf {
        self.transfer_root.join("runtime-state").join(BARRIER)
    }
    pub(super) fn markers(&self, generation: u64) -> Vec<PathBuf> {
        ["mutation-authority", "runtime-mutations"]
            .into_iter()
            .map(|name| {
                self.state_root
                    .join(format!("process-generation-{generation}"))
                    .join(name)
                    .join(BARRIER)
            })
            .collect()
    }
    pub(super) fn base_markers(&self) -> Vec<PathBuf> {
        let mut paths = vec![self.install()];
        paths.extend(
            ["mutation-authority", "runtime-mutations"]
                .map(|name| self.state_root.join(name).join(BARRIER)),
        );
        paths.extend(self.markers(self.born));
        paths
    }
    pub(super) fn verify(&self, paths: impl IntoIterator<Item = PathBuf>) -> Result<()> {
        let bytes = self.bytes()?;
        for path in paths {
            if io::read(&path)? != bytes {
                return Err(hold());
            }
            if path != self.install() {
                let mut entries =
                    std::fs::read_dir(path.parent().ok_or_else(hold)?).map_err(|_| hold())?;
                let entry = entries.next().ok_or_else(hold)?.map_err(|_| hold())?;
                if entry.file_name() != BARRIER || entries.next().is_some() {
                    return Err(hold());
                }
            }
        }
        Ok(())
    }
    pub(super) fn publish(&self, paths: impl IntoIterator<Item = PathBuf>) -> Result<()> {
        let bytes = self.bytes()?;
        for (index, path) in paths.into_iter().enumerate() {
            let _ = index;
            let parent = path.parent().ok_or_else(hold)?;
            if !io::empty(parent)? {
                return Err(hold());
            }
            io::make_directory(parent)?;
            io::create(&path, &bytes)?;
            #[cfg(test)]
            super::tests::checkpoint(&self.state_root, &format!("barrier-{index}"))?;
        }
        Ok(())
    }
}
