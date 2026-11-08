//! Descriptor-relative read-only traversal. No ancestor or leaf symlink follows.
use std::ffi::{CStr, CString};
use std::fs::{File, Metadata};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

use super::{changed, invalid, Budget};
use crate::failure::Result;

pub(super) struct Directory(pub(super) File);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub(super) device: u64,
    pub(super) inode: u64,
    pub(super) bytes: u64,
    mode: u32,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl From<Metadata> for Identity {
    fn from(m: Metadata) -> Self {
        Self {
            device: m.dev(),
            inode: m.ino(),
            bytes: m.len(),
            mode: m.mode(),
            links: m.nlink(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
        }
    }
}

impl Identity {
    pub(super) fn sealed_file(&self) -> bool {
        self.mode & 0o777 == 0o400 && self.links == 1
    }
}

impl Directory {
    pub(super) fn file_identity(&self, name: &str) -> Result<Identity> {
        let file = self.open_at(name, 0)?.ok_or_else(changed)?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(invalid());
        }
        Ok(metadata.into())
    }

    pub(super) fn hash_file(
        &self,
        name: &str,
        maximum: u64,
        budget: &mut Budget,
    ) -> Result<String> {
        use sha2::{Digest, Sha256};
        let mut file = self.open_at(name, 0)?.ok_or_else(changed)?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.is_file()
            || metadata.len() > maximum
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(invalid());
        }
        let before: Identity = metadata.into();
        budget.file(before.bytes)?;
        let mut remaining = before.bytes;
        let mut buffer = [0; 65536];
        let mut hash = Sha256::new();
        while remaining > 0 {
            budget.check()?;
            let size =
                usize::try_from(remaining.min(buffer.len() as u64)).map_err(|_| invalid())?;
            let count = file.read(&mut buffer[..size]).map_err(|_| invalid())?;
            if count == 0 {
                return Err(changed());
            }
            hash.update(&buffer[..count]);
            remaining -= count as u64;
        }
        let current = self.open_at(name, 0)?.ok_or_else(changed)?;
        if Identity::from(file.metadata().map_err(|_| invalid())?) != before
            || Identity::from(current.metadata().map_err(|_| invalid())?) != before
        {
            return Err(changed());
        }
        Ok(crate::command::hex(&hash.finalize()))
    }

    pub(super) fn deserialize<T: serde::de::DeserializeOwned>(
        &self,
        name: &str,
        maximum: u64,
        budget: &mut Budget,
    ) -> Result<(T, String)> {
        use sha2::{Digest, Sha256};
        let file = self.open_at(name, 0)?.ok_or_else(changed)?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.is_file()
            || metadata.len() > maximum
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(invalid());
        }
        let before: Identity = metadata.into();
        if !before.sealed_file() {
            return Err(invalid());
        }
        budget.file(before.bytes)?;
        struct HashReader<R> {
            reader: R,
            hash: Sha256,
        }
        impl<R: Read> Read for HashReader<R> {
            fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
                let read = self.reader.read(bytes)?;
                self.hash.update(&bytes[..read]);
                Ok(read)
            }
        }
        let mut reader = HashReader {
            reader: std::io::BufReader::new((&file).take(maximum + 1)),
            hash: Sha256::new(),
        };
        // Stream decoding: never retain a second manifest-sized byte buffer.
        let value = serde_json::from_reader(&mut reader).map_err(|_| invalid())?;
        let current = self.open_at(name, 0)?.ok_or_else(changed)?;
        if Identity::from(file.metadata().map_err(|_| invalid())?) != before
            || Identity::from(current.metadata().map_err(|_| invalid())?) != before
        {
            return Err(changed());
        }
        budget.check()?;
        Ok((value, crate::command::hex(&reader.hash.finalize())))
    }

    pub(super) fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(invalid());
        }
        let mut directory = Self(File::open("/").map_err(|_| invalid())?);
        for part in path.components() {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => {
                    directory = directory
                        .child(name.to_str().ok_or_else(invalid)?)?
                        .ok_or_else(invalid)?;
                }
                _ => return Err(invalid()),
            }
        }
        Ok(directory)
    }

    pub(super) fn identity(&self) -> Result<Identity> {
        Ok(self.0.metadata().map_err(|_| invalid())?.into())
    }

    pub(super) fn child(&self, name: &str) -> Result<Option<Self>> {
        self.open_at(name, libc::O_DIRECTORY)
            .map(|file| file.map(Self))
    }

    fn open_at(&self, name: &str, flags: i32) -> Result<Option<File>> {
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(invalid());
        }
        let name = CString::new(name).map_err(|_| invalid())?;
        // SAFETY: borrowed directory fd and NUL-terminated component remain live.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK | flags,
            )
        };
        if fd < 0 {
            return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(invalid())
            };
        }
        // SAFETY: openat returned a new owned descriptor.
        Ok(Some(unsafe { File::from_raw_fd(fd) }))
    }

    pub(super) fn names(&self, budget: &mut Budget) -> Result<Vec<String>> {
        // Opening "." creates an independent enumeration offset. dup() would
        // share the original descriptor's offset across a repeated scan.
        let dot = c".";
        // SAFETY: both descriptor and C string are valid for this call.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                dot.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(invalid());
        }
        // SAFETY: fdopendir takes ownership only on success.
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            // SAFETY: failure left fd owned here.
            unsafe {
                libc::close(fd);
            }
            return Err(invalid());
        }
        struct Stream(*mut libc::DIR);
        impl Drop for Stream {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let stream = Stream(stream);
        let mut names = Vec::new();
        loop {
            // Distinguish EOF from a read failure on both supported Unix hosts.
            set_errno(0);
            // SAFETY: the stream is owned for this loop; copy d_name before the next call.
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                if std::io::Error::last_os_error().raw_os_error() != Some(0) {
                    return Err(invalid());
                }
                break;
            }
            // SAFETY: readdir supplies a NUL-terminated d_name.
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
                .to_str()
                .map_err(|_| invalid())?;
            if name == "." || name == ".." {
                continue;
            }
            budget.entry(name.len())?;
            names.push(name.to_owned());
        }
        names.sort();
        Ok(names)
    }

    pub(super) fn read(
        &self,
        name: &str,
        maximum: u64,
        budget: &mut Budget,
    ) -> Result<(Identity, Vec<u8>)> {
        let mut file = self.open_at(name, 0)?.ok_or_else(changed)?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.is_file()
            || metadata.len() > maximum
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(invalid());
        }
        let before: Identity = metadata.into();
        budget.file(before.bytes)?;
        let mut bytes = Vec::with_capacity(usize::try_from(before.bytes).map_err(|_| invalid())?);
        (&mut file)
            .take(maximum + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid())?;
        if bytes.len() as u64 != before.bytes
            || Identity::from(file.metadata().map_err(|_| invalid())?) != before
        {
            return Err(changed());
        }
        // A replaced directory entry must not silently be attributed to the old handle.
        let current = self.open_at(name, 0)?.ok_or_else(changed)?;
        if Identity::from(current.metadata().map_err(|_| invalid())?) != before {
            return Err(changed());
        }
        budget.check()?;
        Ok((before, bytes))
    }
}

fn set_errno(value: i32) {
    // SAFETY: each platform accessor returns this thread's errno pointer.
    #[cfg(target_os = "linux")]
    unsafe {
        *libc::__errno_location() = value;
    }
    #[cfg(target_os = "macos")]
    unsafe {
        *libc::__error() = value;
    }
}
