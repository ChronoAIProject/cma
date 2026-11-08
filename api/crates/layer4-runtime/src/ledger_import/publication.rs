//! Final names never expose unsynced contents. Failed publication is neutralized
//! through the owned descriptor; cleanup never deletes an untrusted entry.
use std::ffi::CString;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;

use super::{invalid, plan_io::storage};
use crate::failure::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    FileSync,
    Rename,
    FinalSync,
    DirectorySync,
    Verify,
    Neutralize,
    NeutralizeSync,
}

pub(super) fn write(
    directory: &File,
    name: &str,
    write: impl FnOnce(&mut File) -> Result<()>,
    verify: impl FnOnce() -> Result<()>,
    mut step: impl FnMut(Step) -> Result<()>,
) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(invalid());
    }
    let name = CString::new(name).map_err(|_| invalid())?;
    let temp =
        CString::new(format!(".publish-{}.tmp", uuid::Uuid::new_v4())).map_err(|_| invalid())?;
    // SAFETY: both names are single components under the retained descriptor.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            temp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(storage());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let result = (|| {
        write(&mut file)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o400))
            .map_err(|_| storage())?;
        step(Step::FileSync)?;
        file.sync_all().map_err(|_| storage())?;
        step(Step::Rename)?;
        rename(directory, &temp, &name)?;
        step(Step::FinalSync)?;
        file.sync_all().map_err(|_| storage())?;
        step(Step::DirectorySync)?;
        directory.sync_all().map_err(|_| storage())?;
        step(Step::Verify)?;
        verify()
    })();
    if let Err(original) = result {
        // No portable unlink-if-inode operation exists. Even a fresh entry
        // identity check followed by unlinkat could delete a replacement. The
        // writable fd still owns our inode after any rename/unlink, including
        // once chmod made it read-only. Leave its empty, invalid artifact in
        // place rather than mutate a name whose current ownership is uncertain.
        if let Err(error) =
            step(Step::Neutralize).and_then(|()| file.set_len(0).map_err(|_| storage()))
        {
            tracing::error!(code = %error.code, phase = "neutralize_descriptor", "ledger publication cleanup failed");
        }
        if let Err(error) =
            step(Step::NeutralizeSync).and_then(|()| file.sync_all().map_err(|_| storage()))
        {
            tracing::error!(code = %error.code, phase = "persist_neutralization", "ledger publication cleanup failed");
        }
        if directory.sync_all().is_err() {
            tracing::error!(
                phase = "persist_withdrawal",
                "ledger publication cleanup failed"
            );
        }
        return Err(original);
    }
    Ok(())
}

fn rename(directory: &File, from: &CString, to: &CString) -> Result<()> {
    let fd = directory.as_raw_fd();
    // SAFETY: descriptor and both component strings remain live. Never replace.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            fd,
            from.as_ptr(),
            fd,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result =
        unsafe { libc::renameatx_np(fd, from.as_ptr(), fd, to.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(storage())
    }
}

#[cfg(test)]
#[path = "publication_tests.rs"]
mod tests;
