//! Descriptor-relative helpers. Every name stays beneath an owned directory FD.
use super::{io_error, rejected};
use crate::failure::{Result, RuntimeError};
use layer3_adapter::MAX_AGENT_FILE_ENTRIES;
use std::{
    ffi::{CStr, CString},
    fs::File,
    io::Read as _,
    os::{
        fd::{AsRawFd as _, FromRawFd as _},
        unix::fs::MetadataExt as _,
    },
};

pub(super) fn descend(root: &File, path: &str, create: bool) -> Result<File> {
    if !layer3_adapter::valid_agent_file_path(path) {
        return Err(rejected("filesystem path is invalid"));
    }
    let mut current = root.try_clone().map_err(io_error)?;
    if path.is_empty() {
        return Ok(current);
    }
    for component in path.split('/') {
        if create {
            mkdir(&current, component)?;
        }
        current = open_at(&current, component, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    }
    Ok(current)
}
pub(super) fn parent(root: &File, path: &str, create: bool) -> Result<(File, String)> {
    if path.is_empty() || !layer3_adapter::valid_agent_file_path(path) {
        return Err(rejected("file path must remain below /home"));
    }
    let (directory, name) = path.rsplit_once('/').unwrap_or(("", path));
    Ok((descend(root, directory, create)?, name.into()))
}
pub(super) fn open_at(parent: &File, name: &str, flags: i32, mode: libc::mode_t) -> Result<File> {
    let name = cstring(name)?;
    // SAFETY: name is one validated component, and parent is a retained directory descriptor.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            libc::c_uint::from(mode),
        )
    };
    if fd < 0 {
        Err(io_error(std::io::Error::last_os_error()))
    } else {
        // SAFETY: openat returned a fresh owned descriptor, transferred once.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
pub(super) fn stat(parent: &File, name: &str) -> Result<libc::stat> {
    let name = cstring(name)?;
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: metadata points to adequate writable storage; path/FD remain live.
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io_error(std::io::Error::last_os_error()));
    }
    // SAFETY: successful fstatat initialized the entire stat structure.
    Ok(unsafe { metadata.assume_init() })
}
pub(super) fn names(directory: &File) -> Result<Vec<String>> {
    use std::os::fd::IntoRawFd as _;
    let fd = directory.try_clone().map_err(io_error)?.into_raw_fd();
    // SAFETY: fdopendir takes ownership of this fresh directory FD on success.
    let stream = unsafe { libc::fdopendir(fd) };
    if stream.is_null() {
        // SAFETY: failure retains ownership of fd; transfer it back into File.
        drop(unsafe { File::from_raw_fd(fd) });
        return Err(io_error(std::io::Error::last_os_error()));
    }
    struct Directory(*mut libc::DIR);
    impl Drop for Directory {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let stream = Directory(stream);
    let mut out = Vec::new();
    loop {
        // SAFETY: stream is valid and only used by this synchronous scope.
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            break;
        }
        // SAFETY: d_name is a NUL-terminated name valid until the next readdir.
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
            .to_str()
            .map_err(|_| rejected("file name is not valid UTF-8"))?;
        if name == "." || name == ".." {
            continue;
        }
        cstring(name)?;
        out.push(name.into());
        if out.len() > MAX_AGENT_FILE_ENTRIES {
            return Err(rejected("directory contains too many entries"));
        }
    }
    Ok(out)
}
pub(super) fn read_regular(parent: &File, name: &str, limit: u64) -> Result<Vec<u8>> {
    let file = open_at(parent, name, libc::O_RDONLY, 0)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(rejected("path is not an single-link regular file"));
    }
    if metadata.len() > limit {
        return Err(RuntimeError::rejected(
            "file_too_large",
            "file exceeds the requested read limit",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > limit {
        return Err(RuntimeError::rejected(
            "file_too_large",
            "file grew beyond the read limit",
        ));
    }
    Ok(bytes)
}
pub(super) fn mkdir(parent: &File, name: &str) -> Result<()> {
    let name = cstring(name)?;
    // SAFETY: name is a single component beneath the retained directory.
    if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } == 0 {
        parent.sync_all().map_err(io_error)?;
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        Ok(())
    } else {
        Err(io_error(error))
    }
}
pub(super) fn rename(parent: &File, from: &str, to: &str) -> Result<()> {
    let from = cstring(from)?;
    let to = cstring(to)?;
    // SAFETY: both names are confined single components under the same retained FD.
    if unsafe {
        libc::renameat(
            parent.as_raw_fd(),
            from.as_ptr(),
            parent.as_raw_fd(),
            to.as_ptr(),
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io_error(std::io::Error::last_os_error()))
    }
}
pub(super) fn unlink(parent: &File, name: &str, directory: bool) -> Result<()> {
    let name = cstring(name)?;
    // SAFETY: unlinkat acts on the named entry without following a symlink.
    if unsafe {
        libc::unlinkat(
            parent.as_raw_fd(),
            name.as_ptr(),
            if directory { libc::AT_REMOVEDIR } else { 0 },
        )
    } == 0
    {
        Ok(())
    } else {
        Err(io_error(std::io::Error::last_os_error()))
    }
}
pub(super) fn cstring(name: &str) -> Result<CString> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(rejected("file component is invalid"));
    }
    CString::new(name).map_err(|_| rejected("file component is invalid"))
}
