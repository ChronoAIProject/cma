//! Read-only, descriptor-relative full-tree capture for a release-fenced snapshot.
//! Never follows symlinks or opens special files. No method mutates source data.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;
use std::{
    fs::{File, Metadata},
    io::Read,
    path::Path,
};

mod attributes;
mod directory;
mod restore;
mod socket;
use directory::Directory;
pub use restore::RestoreTree;
pub const CHUNK_BYTES: usize = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 200_000;
const MAX_DEPTH: usize = 64;
pub type Result<T> = std::result::Result<T, &'static str>;
pub(super) const INVALID: &str = "archive source is unavailable, unsupported or changed";

pub fn read_private_file(path: &Path, limit: usize) -> Result<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let parent = Directory::root(path.parent().ok_or(INVALID)?)?;
    let file = parent.file(path.file_name().ok_or(INVALID)?.as_bytes())?;
    private_bytes(&file, limit)
}
fn private_bytes(file: &File, limit: usize) -> Result<Vec<u8>> {
    let metadata = file.metadata().map_err(|_| INVALID)?;
    if metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.len() > limit as u64
    {
        return Err(INVALID);
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| INVALID)?;
    if bytes.len() > limit {
        return Err(INVALID);
    }
    Ok(bytes)
}

/// Create immutable private evidence through its retained parent descriptor.
/// A retry may reconcile identical bytes but cannot overwrite an earlier result.
pub fn publish_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    publish_private_file_with_sync(path, bytes, File::sync_all, Directory::sync)
}

fn publish_private_file_with_sync(
    path: &Path,
    bytes: &[u8],
    sync_file: impl FnOnce(&File) -> std::io::Result<()>,
    sync_parent: impl FnOnce(&Directory) -> Result<()>,
) -> Result<()> {
    use std::{io::Write, os::unix::ffi::OsStrExt};
    let parent = Directory::root(path.parent().ok_or(INVALID)?)?;
    let metadata = parent.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(INVALID);
    }
    let name = path.file_name().ok_or(INVALID)?.as_bytes();
    let file = if let Some(mut file) = parent.create_private(name)? {
        file.write_all(bytes).map_err(|_| INVALID)?;
        file
    } else {
        let file = parent.file(name)?;
        if private_bytes(&file, bytes.len())? != bytes {
            return Err(INVALID);
        }
        file
    };
    sync_file(&file).map_err(|_| INVALID)?;
    sync_parent(&parent)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceMetadata {
    pub device: u64,
    pub inode: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub length: u64,
    pub links: u64,
    pub modified_seconds: i64,
    pub modified_nanos: i64,
    pub changed_seconds: i64,
    pub changed_nanos: i64,
}
impl From<&Metadata> for SourceMetadata {
    fn from(m: &Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            length: m.len(),
            links: m.nlink(),
            modified_seconds: m.mtime(),
            modified_nanos: m.mtime_nsec(),
            changed_seconds: m.ctime(),
            changed_nanos: m.ctime_nsec(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub attributes: Vec<(String, String)>,
    /// Each component is losslessly encoded; absolute paths never enter object keys.
    pub path: Vec<String>,
    pub metadata: SourceMetadata,
    pub kind: Kind,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Kind {
    Directory,
    File,
    Symlink { target: String },
    Socket,
    Fifo,
}

pub struct SnapshotTree {
    root: Directory,
    pub entries: Vec<Entry>,
}
impl SnapshotTree {
    pub fn open(path: &Path) -> Result<Self> {
        let root = Directory::root(path)?;
        let mut entries = Vec::new();
        walk(&root, &[], &mut entries)?;
        Ok(Self { root, entries })
    }
    pub fn reader(&self, entry: &Entry) -> Result<SourceReader> {
        if entry.kind != Kind::File || entry.path.is_empty() {
            return Err(INVALID);
        }
        let mut parent = self.root.clone_dir()?;
        for component in &entry.path[..entry.path.len() - 1] {
            parent = parent.child(&decode(component)?)?;
        }
        let file = parent.file(&decode(entry.path.last().ok_or(INVALID)?)?)?;
        if SourceMetadata::from(&file.metadata().map_err(|_| INVALID)?) != entry.metadata {
            return Err(INVALID);
        }
        Ok(SourceReader {
            file,
            expected: entry.metadata.clone(),
            read: 0,
        })
    }
    pub fn verify_unchanged(&self) -> Result<()> {
        let mut current = Vec::new();
        walk(&self.root, &[], &mut current)?;
        if current == self.entries {
            Ok(())
        } else {
            Err(INVALID)
        }
    }
}
pub struct SourceReader {
    file: File,
    expected: SourceMetadata,
    read: u64,
}
impl SourceReader {
    pub fn chunk(&mut self) -> Result<Option<Vec<u8>>> {
        let mut bytes = vec![0; CHUNK_BYTES];
        let mut size = 0;
        while size < bytes.len() {
            let n = self.file.read(&mut bytes[size..]).map_err(|_| INVALID)?;
            if n == 0 {
                break;
            }
            size += n;
        }
        self.read = self.read.checked_add(size as u64).ok_or(INVALID)?;
        if self.read > self.expected.length {
            return Err(INVALID);
        }
        if size == 0 {
            if self.read != self.expected.length
                || SourceMetadata::from(&self.file.metadata().map_err(|_| INVALID)?)
                    != self.expected
            {
                return Err(INVALID);
            }
            return Ok(None);
        }
        bytes.truncate(size);
        Ok(Some(bytes))
    }
}
fn decode(value: &str) -> Result<Vec<u8>> {
    let bytes = STANDARD.decode(value).map_err(|_| INVALID)?;
    if bytes.is_empty()
        || bytes == b"."
        || bytes == b".."
        || bytes.contains(&b'/')
        || bytes.contains(&0)
    {
        return Err(INVALID);
    }
    Ok(bytes)
}
fn walk(dir: &Directory, path: &[String], entries: &mut Vec<Entry>) -> Result<()> {
    if path.len() > MAX_DEPTH || entries.len() >= MAX_ENTRIES {
        return Err(INVALID);
    }
    entries.push(Entry {
        attributes: dir.attributes(None)?,
        path: path.to_vec(),
        metadata: SourceMetadata::from(&dir.metadata()?),
        kind: Kind::Directory,
    });
    for name in dir.names()? {
        if entries.len() >= MAX_ENTRIES {
            return Err(INVALID);
        }
        let mut child = path.to_vec();
        child.push(STANDARD.encode(&name));
        let (metadata, kind) = dir.entry(&name)?;
        if kind == Kind::Directory {
            let opened = dir.child(&name)?;
            if SourceMetadata::from(&opened.metadata()?) != metadata {
                return Err(INVALID);
            }
            walk(&opened, &child, entries)?;
        } else {
            entries.push(Entry {
                attributes: dir.attributes(Some(&name))?,
                path: child,
                metadata,
                kind,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
