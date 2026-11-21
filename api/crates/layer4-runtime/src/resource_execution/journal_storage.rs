use std::{
    ffi::{CStr, CString, OsStr},
    fs::{File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::MetadataExt, fs::OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

use layer3_adapter::resource_execution_protocol::{ProtocolError, ProtocolResult};

pub(super) struct Storage {
    parent_path: PathBuf,
    parent: File,
    directory: File,
    marker: File,
    writer: File,
    name: CString,
    marker_name: CString,
}

impl Storage {
    pub(super) fn open(root: &Path) -> ProtocolResult<Self> {
        let parent_path = root.parent().ok_or(ProtocolError::Unknown)?.to_path_buf();
        let name = component(root.file_name())?;
        let marker_name = component(root.with_extension("activated").file_name())?;
        let parent = directory(&parent_path)?;
        let directory = open_at(&parent, &name, libc::O_RDONLY | libc::O_DIRECTORY)?;
        lock(&directory)?;
        let marker = regular(&parent, &marker_name)?;
        lock(&marker)?;
        let writer = regular(&directory, c"writer.lock")?;
        lock(&writer)?;
        let storage = Self {
            parent_path,
            parent,
            directory,
            marker,
            writer,
            name,
            marker_name,
        };
        storage.verify()?;
        Ok(storage)
    }

    pub(super) fn verify(&self) -> ProtocolResult<()> {
        same(&directory(&self.parent_path)?, &self.parent)?;
        same(
            &open_at(&self.parent, &self.name, libc::O_RDONLY | libc::O_DIRECTORY)?,
            &self.directory,
        )?;
        same(&regular(&self.parent, &self.marker_name)?, &self.marker)?;
        same(&regular(&self.directory, c"writer.lock")?, &self.writer)
    }

    pub(super) fn marker(&self) -> ProtocolResult<Vec<u8>> {
        let file = regular(&self.parent, &self.marker_name)?;
        same(&file, &self.marker)?;
        read(file, 4096)
    }

    pub(super) fn state(&self, limit: usize) -> ProtocolResult<Vec<u8>> {
        read(regular(&self.directory, c"state.json")?, limit)
    }

    pub(super) fn create(&self, name: &CStr) -> ProtocolResult<File> {
        self.verify()?;
        open_at(
            &self.directory,
            name,
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
        )
    }

    pub(super) fn publish(&self, temporary: &CStr) -> ProtocolResult<()> {
        self.verify()?;
        // SAFETY: both names and the retained directory descriptor remain live.
        let result = unsafe {
            libc::renameat(
                self.directory.as_raw_fd(),
                temporary.as_ptr(),
                self.directory.as_raw_fd(),
                c"state.json".as_ptr(),
            )
        };
        if result != 0 {
            return Err(ProtocolError::Unavailable);
        }
        Ok(())
    }

    pub(super) fn sync(&self) -> ProtocolResult<()> {
        self.directory
            .sync_all()
            .map_err(|_| ProtocolError::Unavailable)?;
        self.verify()
    }

    pub(super) fn remove_temporary(&self, name: &CStr) {
        // SAFETY: cleanup stays in the retained directory even if its name moved.
        unsafe { libc::unlinkat(self.directory.as_raw_fd(), name.as_ptr(), 0) };
    }
}

fn directory(path: &Path) -> ProtocolResult<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| ProtocolError::Unknown)
}

fn component(name: Option<&OsStr>) -> ProtocolResult<CString> {
    CString::new(name.ok_or(ProtocolError::Unknown)?.as_bytes()).map_err(|_| ProtocolError::Unknown)
}

fn open_at(parent: &File, name: &CStr, flags: i32) -> ProtocolResult<File> {
    // SAFETY: names are single components; the parent and C string remain live.
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0o600 as libc::c_uint,
        )
    };
    if descriptor < 0 {
        return Err(ProtocolError::Unknown);
    }
    // SAFETY: openat returned a fresh descriptor, transferred exactly once.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

fn regular(parent: &File, name: &CStr) -> ProtocolResult<File> {
    let file = open_at(parent, name, libc::O_RDONLY)?;
    let metadata = file.metadata().map_err(|_| ProtocolError::Unknown)?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(ProtocolError::Unknown);
    }
    Ok(file)
}

fn same(named: &File, retained: &File) -> ProtocolResult<()> {
    let named = named.metadata().map_err(|_| ProtocolError::Unknown)?;
    let retained = retained.metadata().map_err(|_| ProtocolError::Unknown)?;
    if named.dev() != retained.dev() || named.ino() != retained.ino() {
        return Err(ProtocolError::Unknown);
    }
    Ok(())
}

fn lock(file: &File) -> ProtocolResult<()> {
    // SAFETY: flock operates only on the descriptor retained by this storage.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(ProtocolError::Unavailable);
    }
    Ok(())
}

fn read(file: File, limit: usize) -> ProtocolResult<Vec<u8>> {
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ProtocolError::Unknown)?;
    if bytes.len() > limit {
        return Err(ProtocolError::Capacity);
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "journal_storage_tests.rs"]
mod tests;
