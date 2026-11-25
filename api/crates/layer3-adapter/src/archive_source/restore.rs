//! Reconstruction only into a new private directory. No execution or replay.
use super::*;
use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::{
    ffi::OsStringExt,
    fs::{DirBuilderExt, OpenOptionsExt},
};
use std::path::PathBuf;

pub struct RestoreTree {
    root: PathBuf,
    expected: BTreeMap<Vec<String>, Kind>,
    links: BTreeMap<(u64, u64), PathBuf>,
    failed: bool,
}
impl RestoreTree {
    pub fn create(path: &Path) -> Result<Self> {
        // Resolve ancestors without following links before creating an exclusive
        // directory. The private operator account must also own its parent.
        let parent = Directory::root(path.parent().ok_or(INVALID)?)?;
        let metadata = parent.metadata()?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err(INVALID);
        }
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|_| INVALID)?;
        Directory::root(path)?;
        Ok(Self {
            root: path.to_path_buf(),
            expected: BTreeMap::new(),
            links: BTreeMap::new(),
            failed: false,
        })
    }
    pub fn path(&self) -> &Path {
        &self.root
    }
    fn destination(&self, entry: &Entry) -> Result<PathBuf> {
        let mut path = self.root.clone();
        for part in &entry.path {
            path.push(std::ffi::OsString::from_vec(decode(part)?));
        }
        if !entry.path.is_empty() {
            Directory::root(path.parent().ok_or(INVALID)?)?;
        }
        Ok(path)
    }
    pub fn add(&mut self, entry: &Entry) -> Result<Option<File>> {
        let result = self.add_entry(entry);
        self.failed |= result.is_err();
        result
    }
    fn add_entry(&mut self, entry: &Entry) -> Result<Option<File>> {
        if self
            .expected
            .insert(entry.path.clone(), entry.kind.clone())
            .is_some()
        {
            return Err(INVALID);
        }
        let path = self.destination(entry)?;
        match &entry.kind {
            Kind::Directory => {
                if !entry.path.is_empty() {
                    std::fs::DirBuilder::new()
                        .mode(0o700)
                        .create(path)
                        .map_err(|_| INVALID)?;
                }
                Ok(None)
            }
            Kind::File => {
                if entry.path.is_empty() {
                    return Err(INVALID);
                }
                let identity = (entry.metadata.device, entry.metadata.inode);
                if let Some(first) = self.links.get(&identity) {
                    std::fs::hard_link(first, &path).map_err(|_| INVALID)?;
                    // Verify this alias independently; never write through it.
                    Ok(None)
                } else {
                    let file = std::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .mode(0o600)
                        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                        .open(&path)
                        .map_err(|_| INVALID)?;
                    self.links.insert(identity, path);
                    Ok(Some(file))
                }
            }
            Kind::Symlink { target } => {
                let target = STANDARD.decode(target).map_err(|_| INVALID)?;
                if target.contains(&0) {
                    return Err(INVALID);
                }
                std::os::unix::fs::symlink(std::ffi::OsString::from_vec(target), path)
                    .map_err(|_| INVALID)?;
                Ok(None)
            }
            // Sockets are ephemeral processes, not durable file data. The
            // manifest retains their metadata; reconstruct a closed socket.
            Kind::Socket => {
                let parent = Directory::root(path.parent().ok_or(INVALID)?)?;
                parent.socket(&decode(entry.path.last().ok_or(INVALID)?)?)?;
                Ok(None)
            }
            Kind::Fifo => {
                use std::os::unix::ffi::OsStrExt;
                let path =
                    std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| INVALID)?;
                if unsafe { libc::mkfifo(path.as_ptr(), 0o600) } != 0 {
                    return Err(INVALID);
                }
                Ok(None)
            }
        }
    }
    pub fn verify_file(&self, entry: &Entry, expected_sha: &str) -> Result<()> {
        use sha2::{Digest, Sha256};
        let path = self.destination(entry)?;
        let directory = Directory::root(path.parent().ok_or(INVALID)?)?;
        let file = directory.file(&decode(entry.path.last().ok_or(INVALID)?)?)?;
        let metadata = SourceMetadata::from(&file.metadata().map_err(|_| INVALID)?);
        if metadata.length != entry.metadata.length {
            return Err(INVALID);
        }
        let mut reader = SourceReader {
            file,
            expected: metadata,
            read: 0,
        };
        let mut hash = Sha256::new();
        while let Some(chunk) = reader.chunk()? {
            hash.update(chunk);
        }
        if format!("{:x}", hash.finalize()) != expected_sha {
            return Err(INVALID);
        }
        File::open(path.parent().ok_or(INVALID)?)
            .map_err(|_| INVALID)?
            .sync_all()
            .map_err(|_| INVALID)
    }
    pub fn finish(&self) -> Result<()> {
        if self.failed {
            return Err(INVALID);
        }
        let tree = SnapshotTree::open(&self.root)?;
        let found = tree
            .entries
            .into_iter()
            .map(|e| (e.path, e.kind))
            .collect::<BTreeMap<_, _>>();
        if found != self.expected {
            return Err(INVALID);
        }
        File::open(&self.root)
            .map_err(|_| INVALID)?
            .sync_all()
            .map_err(|_| INVALID)
    }
    pub fn write_chunk(file: &mut File, bytes: &[u8]) -> Result<()> {
        file.write_all(bytes).map_err(|_| INVALID)
    }
}
