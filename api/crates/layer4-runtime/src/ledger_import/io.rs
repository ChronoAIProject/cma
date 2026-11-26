//! Descriptor-relative read-only traversal. No ancestor or leaf symlink follows.
use std::ffi::{CStr, CString};
use std::fs::{File, Metadata};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

use super::{changed, invalid, invalid_guard, invalid_io, Budget};
use crate::failure::Result;

pub(crate) struct Directory(pub(super) File);

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub(super) device: u64,
    pub(super) inode: u64,
    pub(crate) bytes: u64,
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
    pub(super) fn sync_file(&self, name: &str) -> Result<()> {
        self.file_identity(name)?;
        self.open_at(name, 0)?
            .ok_or_else(changed)?
            .sync_all()
            .map_err(|_| super::plan_io::storage())
    }

    pub(super) fn file_identity(&self, name: &str) -> Result<Identity> {
        let file = self.open_at(name, 0)?.ok_or_else(changed)?;
        let metadata = file
            .metadata()
            .map_err(|e| invalid_io("io.file_identity.fstat", e))?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(invalid_guard("io.file_identity.private_regular_file"));
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
        let metadata = file
            .metadata()
            .map_err(|e| invalid_io("io.hash_file.fstat", e))?;
        if !metadata.is_file()
            || metadata.len() > maximum
            || metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(invalid_guard("io.hash_file.private_bounded_regular_file"));
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
            let count = file
                .read(&mut buffer[..size])
                .map_err(|e| invalid_io("io.hash_file.read", e))?;
            if count == 0 {
                return Err(changed());
            }
            hash.update(&buffer[..count]);
            remaining -= count as u64;
        }
        let current = self.open_at(name, 0)?.ok_or_else(changed)?;
        if Identity::from(
            file.metadata()
                .map_err(|e| invalid_io("io.hash_file.retained_fstat", e))?,
        ) != before
            || Identity::from(
                current
                    .metadata()
                    .map_err(|e| invalid_io("io.hash_file.current_fstat", e))?,
            ) != before
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
        let metadata = file
            .metadata()
            .map_err(|e| invalid_io("io.deserialize.fstat", e))?;
        if !metadata.is_file()
            || metadata.len() > maximum
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(invalid_guard("io.deserialize.owned_bounded_regular_file"));
        }
        let before: Identity = metadata.into();
        if !before.sealed_file() {
            return Err(invalid_guard("io.deserialize.sealed_file"));
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
        if Identity::from(
            file.metadata()
                .map_err(|e| invalid_io("io.deserialize.retained_fstat", e))?,
        ) != before
            || Identity::from(
                current
                    .metadata()
                    .map_err(|e| invalid_io("io.deserialize.current_fstat", e))?,
            ) != before
        {
            return Err(changed());
        }
        budget.check()?;
        Ok((value, crate::command::hex(&reader.hash.finalize())))
    }

    pub(crate) fn open(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(invalid_guard("io.open.absolute_path"));
        }
        let mut directory = Self(File::open("/").map_err(|e| invalid_io("io.open.root", e))?);
        for part in path.components() {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => {
                    directory = directory
                        .child(
                            name.to_str()
                                .ok_or_else(|| invalid_guard("io.open.utf8_component"))?,
                        )?
                        .ok_or_else(|| invalid_guard("io.open.existing_component"))?;
                }
                _ => return Err(invalid_guard("io.open.normal_component")),
            }
        }
        Ok(directory)
    }

    pub(crate) fn identity(&self) -> Result<Identity> {
        Ok(self
            .0
            .metadata()
            .map_err(|e| invalid_io("io.identity.fstat", e))?
            .into())
    }

    pub(crate) fn child(&self, name: &str) -> Result<Option<Self>> {
        self.open_at(name, libc::O_DIRECTORY)
            .map(|file| file.map(Self))
    }

    pub(crate) fn open_file(&self, name: &str) -> Result<Option<File>> {
        self.open_at(name, 0)
    }

    pub(super) fn open_at(&self, name: &str, flags: i32) -> Result<Option<File>> {
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(invalid_guard("io.open_at.single_component"));
        }
        let name =
            CString::new(name).map_err(|_| invalid_guard("io.open_at.nul_free_component"))?;
        // SAFETY: borrowed directory fd and NUL-terminated component remain live.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK | flags,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(invalid_io("io.open_at.openat", error))
            };
        }
        // SAFETY: openat returned a new owned descriptor.
        Ok(Some(unsafe { File::from_raw_fd(fd) }))
    }

    pub(crate) fn names(&self, budget: &mut Budget) -> Result<Vec<String>> {
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
            return Err(invalid_io(
                "io.names.openat",
                std::io::Error::last_os_error(),
            ));
        }
        // SAFETY: fdopendir takes ownership only on success.
        let stream = unsafe { libc::fdopendir(fd) };
        if stream.is_null() {
            let error = std::io::Error::last_os_error();
            // SAFETY: failure left fd owned here.
            if unsafe { libc::close(fd) } != 0 {
                tracing::error!(
                    phase = "open_directory_stream",
                    "held ledger import descriptor cleanup failed"
                );
            }
            return Err(invalid_io("io.names.fdopendir", error));
        }
        struct Stream(*mut libc::DIR);
        impl Drop for Stream {
            fn drop(&mut self) {
                // SAFETY: this stream is owned here and closed exactly once.
                if unsafe { libc::closedir(self.0) } != 0 {
                    tracing::error!(
                        phase = "read_directory_stream",
                        "held ledger import directory cleanup failed"
                    );
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
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(0) {
                    return Err(invalid_io("io.names.readdir", error));
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

    pub(crate) fn read(
        &self,
        name: &str,
        maximum: u64,
        budget: &mut Budget,
    ) -> Result<(Identity, Vec<u8>)> {
        self.read_checked(name, maximum, budget, false)
    }

    pub(crate) fn read_only_record(
        &self,
        name: &str,
        maximum: u64,
        budget: &mut Budget,
    ) -> Result<(Identity, Vec<u8>)> {
        self.read_checked(name, maximum, budget, true)
    }

    fn read_checked(
        &self,
        name: &str,
        maximum: u64,
        budget: &mut Budget,
        read_only: bool,
    ) -> Result<(Identity, Vec<u8>)> {
        let mut file = self.open_at(name, 0)?.ok_or_else(changed)?;
        if read_only && !mounted_read_only(&file)? {
            return Err(invalid());
        }
        let metadata = file
            .metadata()
            .map_err(|e| invalid_io("io.read.fstat", e))?;
        if !metadata.is_file()
            || metadata.len() > maximum
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(invalid_guard("io.read.owned_bounded_regular_file"));
        }
        let before: Identity = metadata.into();
        budget.file(before.bytes)?;
        let mut bytes = Vec::with_capacity(usize::try_from(before.bytes).map_err(|_| invalid())?);
        (&mut file)
            .take(maximum + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| invalid_io("io.read.read_to_end", e))?;
        if bytes.len() as u64 != before.bytes
            || Identity::from(
                file.metadata()
                    .map_err(|e| invalid_io("io.read.retained_fstat", e))?,
            ) != before
        {
            return Err(changed());
        }
        // A replaced directory entry must not silently be attributed to the old handle.
        let current = self.open_at(name, 0)?.ok_or_else(changed)?;
        if Identity::from(
            current
                .metadata()
                .map_err(|e| invalid_io("io.read.current_fstat", e))?,
        ) != before
        {
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

pub(crate) fn mounted_read_only(file: &File) -> Result<bool> {
    let mut status = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: file owns a live descriptor and fstatvfs initializes status on success.
    if unsafe { libc::fstatvfs(file.as_raw_fd(), status.as_mut_ptr()) } != 0 {
        return Err(invalid_io(
            "io.mount.fstatvfs",
            std::io::Error::last_os_error(),
        ));
    }
    // SAFETY: the successful call initialized status.
    Ok(unsafe { status.assume_init() }.f_flag & libc::ST_RDONLY != 0)
}

impl Directory {
    pub(crate) fn mounted_read_only(&self) -> Result<bool> {
        mounted_read_only(&self.0)
    }
}
