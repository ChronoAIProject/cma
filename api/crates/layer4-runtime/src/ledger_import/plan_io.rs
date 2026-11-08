//! Create-only, descriptor-relative output. Never open a source file for writing.
use std::ffi::CString;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::io::Directory;
use super::{capacity, changed, invalid};
use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};

pub(super) struct Archive {
    directory: Directory,
    parent: Directory,
    path: PathBuf,
    identity: (u64, u64),
}

impl Archive {
    pub(super) fn create(config: &RuntimeConfig, path: &Path) -> Result<Self> {
        // A source root must not contain the archive, or vice versa. Opening
        // each component below also rejects aliases through symbolic links.
        if [&config.home, &config.state_root, &config.checkpoint_root]
            .iter()
            .any(|root| path.starts_with(root) || root.starts_with(path))
        {
            return Err(invalid());
        }
        let parent = Directory::open(path.parent().ok_or_else(invalid)?)?;
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(invalid)?;
        let name_c = CString::new(name).map_err(|_| invalid())?;
        // SAFETY: the parent fd and one NUL-terminated component are live.
        if unsafe { libc::mkdirat(parent.0.as_raw_fd(), name_c.as_ptr(), 0o700) } != 0 {
            return Err(storage());
        }
        parent.0.sync_all().map_err(|_| storage())?;
        let directory = parent.child(name)?.ok_or_else(changed)?;
        let metadata = directory.0.metadata().map_err(|_| storage())?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(invalid());
        }
        let identity = (metadata.dev(), metadata.ino());
        let result = Self {
            directory,
            parent,
            path: path.into(),
            identity,
        };
        result.verify()?;
        Ok(result)
    }

    pub(super) fn write(
        &self,
        name: &str,
        write: impl FnOnce(&mut File) -> Result<()>,
    ) -> Result<()> {
        self.verify()?;
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(invalid());
        }
        let name = CString::new(name).map_err(|_| invalid())?;
        // SAFETY: create a new file under the owned directory, never following
        // or replacing any existing name. The returned descriptor is owned.
        let fd = unsafe {
            libc::openat(
                self.directory.0.as_raw_fd(),
                name.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(storage());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        write(&mut file)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o400))
            .map_err(|_| storage())?;
        file.sync_all().map_err(|_| storage())?;
        self.directory.0.sync_all().map_err(|_| storage())?;
        self.verify()
    }

    pub(super) fn bytes(&self, name: &str, bytes: &[u8]) -> Result<()> {
        self.write(name, |file| file.write_all(bytes).map_err(|_| storage()))
    }

    pub(super) fn json(&self, name: &str, value: &impl serde::Serialize) -> Result<String> {
        self.json_bounded(name, value, 24 * 1024 * 1024)
    }

    pub(super) fn json_bounded(
        &self,
        name: &str,
        value: &impl serde::Serialize,
        maximum: u64,
    ) -> Result<String> {
        let mut digest = Sha256::new();
        struct HashWriter<'a> {
            file: &'a mut File,
            digest: &'a mut Sha256,
            remaining: u64,
        }
        impl Write for HashWriter<'_> {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if bytes.len() as u64 > self.remaining {
                    return Err(std::io::Error::from(std::io::ErrorKind::FileTooLarge));
                }
                let length = self.file.write(bytes)?;
                self.digest.update(&bytes[..length]);
                self.remaining -= length as u64;
                Ok(length)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                self.file.flush()
            }
        }
        self.write(name, |file| {
            serde_json::to_writer(
                HashWriter {
                    file,
                    digest: &mut digest,
                    remaining: maximum,
                },
                value,
            )
            .map_err(|error| {
                if error.io_error_kind() == Some(std::io::ErrorKind::FileTooLarge) {
                    capacity()
                } else {
                    storage()
                }
            })
        })?;
        Ok(crate::command::hex(&digest.finalize()))
    }

    pub(super) fn verify(&self) -> Result<()> {
        let directory = Directory::open(&self.path)?;
        let metadata = directory.0.metadata().map_err(|_| storage())?;
        if (metadata.dev(), metadata.ino()) != self.identity {
            return Err(changed());
        }
        let parent = Directory::open(self.path.parent().ok_or_else(invalid)?)?;
        let original = self.parent.identity()?;
        let current = parent.identity()?;
        if (original.device, original.inode) != (current.device, current.inode) {
            return Err(changed());
        }
        Ok(())
    }
}

fn storage() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_storage_hold",
        "runtime mutation ledger storage requires verification",
    )
}
