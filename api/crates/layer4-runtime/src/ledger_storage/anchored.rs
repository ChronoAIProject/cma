//! Detached import storage rooted in an owned directory descriptor. No pathname
//! lookup can redirect database, rollback-journal or reserve writes elsewhere.
mod io;
pub(crate) mod probe;
mod vfs;
use super::{connection, invalid, reserve, storage_error};
use crate::failure::Result;
use rusqlite::{Connection, OpenFlags};
use std::ffi::CStr;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::sync::Arc;
pub(super) use vfs::Vfs;

pub(super) fn file(directory: &File, name: &CStr, flags: i32) -> std::io::Result<File> {
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let meta = file.metadata()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.mode() & 0o077 != 0
        || meta.uid() != unsafe { libc::geteuid() }
    {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    Ok(file)
}

pub(super) fn open(directory: Arc<File>, pages: u32) -> Result<(Connection, File, Vfs)> {
    let meta = directory.metadata().map_err(|_| storage_error())?;
    if !meta.is_dir()
        || meta.mode() & 0o077 != 0
        || meta.uid() != unsafe { libc::geteuid() }
        || pages < 16
        || pages > i32::MAX as u32
    {
        return Err(invalid());
    }
    let create = libc::O_RDWR | libc::O_CREAT | libc::O_EXCL;
    let lock = file(&directory, c"writer.lock", create).map_err(|_| storage_error())?;
    lock.try_lock().map_err(|_| storage_error())?;
    let database = file(&directory, c"ledger.sqlite3", create).map_err(|_| storage_error())?;
    database.sync_all().map_err(|_| storage_error())?;
    drop(database);
    let mut reserved =
        file(&directory, c"emergency.reserve", create).map_err(|_| storage_error())?;
    let block = [0xa5; 64 * 1024];
    for _ in 0..reserve::BYTES / block.len() as u64 {
        reserved.write_all(&block).map_err(|_| storage_error())?;
    }
    reserved.sync_all().map_err(|_| storage_error())?;
    directory.sync_all().map_err(|_| storage_error())?;
    let vfs = Vfs::new(directory)?;
    let connection = Connection::open_with_flags_and_vfs(
        "ledger.sqlite3",
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        vfs.name(),
    )
    .map_err(|_| storage_error())?;
    connection::configure(&connection, pages)?;
    Ok((connection, lock, vfs))
}

pub(crate) fn require(directory: &File) -> Result<()> {
    let reserved =
        file(directory, c"emergency.reserve", libc::O_RDONLY).map_err(|_| storage_error())?;
    let meta = reserved.metadata().map_err(|_| storage_error())?;
    if meta.len() != reserve::BYTES
        || meta
            .blocks()
            .checked_mul(512)
            .is_none_or(|n| n < reserve::BYTES)
    {
        return Err(storage_error());
    }
    Ok(())
}

pub(super) fn release(directory: &File) -> Result<()> {
    let file = file(directory, c"emergency.reserve", libc::O_RDWR).map_err(|_| storage_error())?;
    file.set_len(0)
        .and_then(|()| file.sync_all())
        .map_err(|_| storage_error())
}

#[cfg(test)]
mod tests;
