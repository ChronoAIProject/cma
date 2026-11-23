use std::collections::BTreeMap;
use std::fs::File;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::{error, sha256, Instant, REQUEST_BYTES};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::ledger_import::io::{mounted_read_only, Directory, Identity};
use crate::ledger_import::Budget;

pub(super) const HOME: usize = 0;
pub(super) const STATE: usize = 1;
pub(super) const TRANSFER: usize = 2;

struct ObservedDirectory {
    handle: Directory,
    identity: Identity,
    names: Vec<String>,
    path: PathBuf,
}

struct ObservedFile {
    directory: usize,
    name: String,
    identity: Identity,
    contents: Option<(u64, String)>,
}

pub(super) struct Source {
    roots: [PathBuf; 3],
    directories: Vec<ObservedDirectory>,
    paths: BTreeMap<(usize, String), usize>,
    files: Vec<ObservedFile>,
    pub(super) budget: Budget,
    start: Instant,
    #[cfg(test)]
    pub(super) elapsed: Option<Duration>,
    #[cfg(test)]
    pub(super) allow_writable_fixture: bool,
}

impl Source {
    pub(super) fn open(config: &RuntimeConfig, budget: Budget) -> Result<Self> {
        Self::at(
            [
                config.home.clone(),
                config.state_root.clone(),
                config.checkpoint_root.clone(),
            ],
            budget,
            false,
        )
    }

    fn at(roots: [PathBuf; 3], budget: Budget, fixture: bool) -> Result<Self> {
        #[cfg(not(test))]
        assert!(!fixture);
        let mut source = Self {
            roots,
            directories: Vec::new(),
            paths: BTreeMap::new(),
            files: Vec::new(),
            budget,
            start: Instant::now(),
            #[cfg(test)]
            elapsed: None,
            #[cfg(test)]
            allow_writable_fixture: fixture,
        };
        for root in 0..3 {
            let handle = Directory::open(&source.roots[root]).map_err(safe)?;
            source.add_directory(root, "", source.roots[root].clone(), handle)?;
        }
        Ok(source)
    }

    #[cfg(test)]
    pub(super) fn fixture(roots: [PathBuf; 3], budget: Budget) -> Result<Self> {
        Self::at(roots, budget, true)
    }

    fn require_mount(&self) -> bool {
        #[cfg(test)]
        if self.allow_writable_fixture {
            return false;
        }
        true
    }

    pub(super) fn check(&self) -> Result<()> {
        let elapsed = self.start.elapsed();
        #[cfg(test)]
        let elapsed = self.elapsed.unwrap_or(elapsed);
        if elapsed > Duration::from_secs(30) {
            return Err(error("time_budget"));
        }
        self.budget.check().map_err(safe)
    }

    fn add_directory(
        &mut self,
        root: usize,
        relative: &str,
        path: PathBuf,
        handle: Directory,
    ) -> Result<usize> {
        self.check()?;
        if self.require_mount() && !handle.mounted_read_only().map_err(safe)? {
            return Err(error("writable_mount"));
        }
        let identity = handle.identity().map_err(safe)?;
        // Home contents are deliberately unassessed, including their names.
        let names = if root == HOME && relative.is_empty() {
            Vec::new()
        } else {
            handle.names(&mut self.budget).map_err(safe)?
        };
        if handle.identity().map_err(safe)? != identity {
            return Err(error("source_changed"));
        }
        let index = self.directories.len();
        self.directories.push(ObservedDirectory {
            handle,
            identity,
            names,
            path,
        });
        self.paths.insert((root, relative.into()), index);
        Ok(index)
    }

    fn directory(&mut self, root: usize, relative: &str) -> Result<Option<usize>> {
        self.check()?;
        if let Some(index) = self.paths.get(&(root, relative.into())) {
            return Ok(Some(*index));
        }
        let (parent, name) = relative.rsplit_once('/').unwrap_or(("", relative));
        let Some(parent) = self.directory(root, parent)? else {
            return Ok(None);
        };
        let Some(handle) = self.directories[parent].handle.child(name).map_err(safe)? else {
            if self.directories[parent]
                .names
                .iter()
                .any(|entry| entry == name)
            {
                return Err(error("source_changed"));
            }
            return Ok(None);
        };
        if !self.directories[parent]
            .names
            .iter()
            .any(|entry| entry == name)
        {
            return Err(error("source_changed"));
        }
        self.add_directory(root, relative, self.roots[root].join(relative), handle)
            .map(Some)
    }

    pub(super) fn names(&mut self, root: usize, relative: &str) -> Result<Option<Vec<String>>> {
        Ok(self
            .directory(root, relative)?
            .map(|index| self.directories[index].names.clone()))
    }

    pub(super) fn present(&mut self, root: usize, relative: &str) -> Result<bool> {
        let (parent, name) = relative.rsplit_once('/').unwrap_or(("", relative));
        Ok(self
            .names(root, parent)?
            .is_some_and(|names| names.iter().any(|entry| entry == name)))
    }

    pub(super) fn record(
        &mut self,
        root: usize,
        relative: &str,
        maximum: u64,
    ) -> Result<Option<Vec<u8>>> {
        let Some((directory, name, identity)) = self.observe_file(root, relative)? else {
            return Ok(None);
        };
        if identity.bytes > maximum {
            return Err(error("record_budget"));
        }
        let (read_identity, bytes) = if self.require_mount() {
            self.directories[directory]
                .handle
                .read_only_record(&name, maximum, &mut self.budget)
        } else {
            self.directories[directory]
                .handle
                .read(&name, maximum, &mut self.budget)
        }
        .map_err(safe)?;
        if read_identity != identity {
            return Err(error("source_changed"));
        }
        self.files.push(ObservedFile {
            directory,
            name,
            identity,
            contents: Some((maximum, sha256(&bytes))),
        });
        self.check()?;
        Ok(Some(bytes))
    }

    pub(super) fn file_size(&mut self, root: usize, relative: &str) -> Result<Option<u64>> {
        let Some((directory, name, identity)) = self.observe_file(root, relative)? else {
            return Ok(None);
        };
        let size = identity.bytes;
        self.files.push(ObservedFile {
            directory,
            name,
            identity,
            contents: None,
        });
        Ok(Some(size))
    }

    fn observe_file(
        &mut self,
        root: usize,
        relative: &str,
    ) -> Result<Option<(usize, String, Identity)>> {
        self.check()?;
        let (parent, name) = relative.rsplit_once('/').unwrap_or(("", relative));
        let Some(directory) = self.directory(root, parent)? else {
            return Ok(None);
        };
        let file = self.directories[directory]
            .handle
            .open_file(name)
            .map_err(safe)?;
        let expected = self.directories[directory]
            .names
            .iter()
            .any(|entry| entry == name);
        if file.is_some() != expected {
            return Err(error("source_changed"));
        }
        file.map(|file| {
            checked_file(&file, self.require_mount())
                .map(|identity| (directory, name.into(), identity))
        })
        .transpose()
    }

    pub(super) fn finish(&mut self, request_digest: &str) -> Result<String> {
        self.check()?;
        let mut transcript = sha2::Sha256::new();
        use sha2::Digest;
        transcript.update(b"chrono.restored-home.assessed-metadata.v1\0");
        transcript.update(request_digest.as_bytes());
        for (index, observed) in self.directories.iter().enumerate() {
            let current = Directory::open(&observed.path).map_err(safe)?;
            if self.require_mount() && !current.mounted_read_only().map_err(safe)? {
                return Err(error("writable_mount"));
            }
            if current.identity().map_err(safe)? != observed.identity
                || observed.handle.identity().map_err(safe)? != observed.identity
            {
                return Err(error("source_changed"));
            }
            if index != HOME && current.names(&mut self.budget).map_err(safe)? != observed.names {
                return Err(error("source_changed"));
            }
            transcript.update(
                serde_json::to_vec(&(index, &observed.identity, &observed.names))
                    .map_err(|_| error("metadata_encoding"))?,
            );
        }
        for observed in &self.files {
            let directory = &self.directories[observed.directory].handle;
            let file = directory
                .open_file(&observed.name)
                .map_err(safe)?
                .ok_or_else(|| error("source_changed"))?;
            if checked_file(&file, self.require_mount())? != observed.identity {
                return Err(error("source_changed"));
            }
            if let Some((maximum, digest)) = &observed.contents {
                let (identity, bytes) = if self.require_mount() {
                    directory.read_only_record(&observed.name, *maximum, &mut self.budget)
                } else {
                    directory.read(&observed.name, *maximum, &mut self.budget)
                }
                .map_err(safe)?;
                if identity != observed.identity || sha256(&bytes) != *digest {
                    return Err(error("source_changed"));
                }
            }
            transcript.update(
                serde_json::to_vec(&(
                    observed.directory,
                    &observed.name,
                    &observed.identity,
                    &observed.contents,
                ))
                .map_err(|_| error("metadata_encoding"))?,
            );
        }
        self.check()?;
        Ok(crate::command::hex(&transcript.finalize()))
    }
}

fn checked_file(file: &File, require_mount: bool) -> Result<Identity> {
    if require_mount && !mounted_read_only(file).map_err(safe)? {
        return Err(error("writable_mount"));
    }
    let metadata = file.metadata().map_err(|_| error("io"))?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(error("unsafe_file"));
    }
    Ok(metadata.into())
}

pub(super) fn request(path: &Path, budget: &mut Budget) -> Result<Vec<u8>> {
    if !path.is_absolute() {
        return Err(error("request_path"));
    }
    let parent =
        Directory::open(path.parent().ok_or_else(|| error("request_path"))?).map_err(safe)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| error("request_path"))?;
    let (_, bytes) = parent.read(name, REQUEST_BYTES, budget).map_err(safe)?;
    Ok(bytes)
}

fn safe(failure: crate::failure::RuntimeError) -> crate::failure::RuntimeError {
    error(match failure.code.as_str() {
        "mutation_ledger_migration_capacity" => "metadata_budget",
        "mutation_ledger_migration_changed" => "source_changed",
        _ => "io",
    })
}
