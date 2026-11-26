//! Reconstruct a closed socket without putting its final pathname in sockaddr_un.
use super::{File, MetadataExt, Result, INVALID};
use std::{
    ffi::{CStr, CString},
    os::fd::{AsRawFd, FromRawFd},
};

pub(super) fn restore(parent: &File, name: &[u8]) -> Result<()> {
    reconstruct(parent, name, bind_closed)
}

fn reconstruct(parent: &File, name: &[u8], bind: impl FnOnce(&File) -> Result<()>) -> Result<()> {
    if name.is_empty() || name == b"." || name == b".." || name.contains(&b'/') {
        return Err(INVALID);
    }
    let name = CString::new(name).map_err(|_| INVALID)?;
    let mut scratch = Scratch::create(parent)?;
    let placed = bind(&scratch.directory).and_then(|()| place(&scratch.directory, parent, &name));
    let cleaned = scratch.cleanup();
    placed?;
    cleaned?;
    parent.sync_all().map_err(|_| INVALID)
}

struct Scratch<'a> {
    parent: &'a File,
    name: CString,
    directory: File,
    removed: bool,
}

impl<'a> Scratch<'a> {
    fn create(parent: &'a File) -> Result<Self> {
        let metadata = parent.metadata().map_err(|_| INVALID)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(INVALID);
        }
        for _ in 0..8 {
            let name = CString::new(format!(".cma-socket-{}", uuid::Uuid::new_v4().simple()))
                .map_err(|_| INVALID)?;
            if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST) {
                    continue;
                }
                return Err(INVALID);
            }
            let fd = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) };
                return Err(INVALID);
            }
            return Ok(Self {
                parent,
                name,
                directory: unsafe { File::from_raw_fd(fd) },
                removed: false,
            });
        }
        Err(INVALID)
    }

    fn cleanup(&mut self) -> Result<()> {
        if self.removed {
            return Ok(());
        }
        if unsafe { libc::unlinkat(self.directory.as_raw_fd(), c"s".as_ptr(), 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT)
        {
            return Err(INVALID);
        }
        if unsafe {
            libc::unlinkat(
                self.parent.as_raw_fd(),
                self.name.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        } != 0
        {
            return Err(INVALID);
        }
        self.removed = true;
        Ok(())
    }
}

impl Drop for Scratch<'_> {
    fn drop(&mut self) {
        // Explicit cleanup is checked before success; Drop only covers unwinding.
        let _ = self.cleanup();
    }
}

fn bind_closed(directory: &File) -> Result<()> {
    let directory = directory.as_raw_fd();
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    address.sun_path[0] = b's' as libc::c_char;
    let length = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + 2) as libc::socklen_t;
    #[cfg(target_os = "macos")]
    {
        address.sun_len = length as u8;
    }
    // Only the child changes cwd. After fork it uses exclusively async-signal-safe
    // POSIX calls: no allocation, Rust destructor, logging, exec, or unwinding.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(INVALID);
    }
    if pid == 0 {
        unsafe {
            if libc::fchdir(directory) != 0 {
                libc::_exit(1);
            }
            let socket = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
            if socket < 0 {
                libc::_exit(1);
            }
            let bound = libc::bind(socket, (&raw const address).cast(), length);
            let closed = libc::close(socket);
            libc::_exit(i32::from(bound != 0 || closed != 0));
        }
    }
    // This synchronous operation cannot be dropped at an async suspension point.
    // Reap the exact child before moving its closed node or removing its directory.
    let mut status = 0;
    loop {
        if unsafe { libc::waitpid(pid, &mut status, 0) } == pid {
            return if libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0 {
                Ok(())
            } else {
                Err(INVALID)
            };
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return Err(INVALID);
        }
    }
}

fn place(source: &File, parent: &File, name: &CStr) -> Result<()> {
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            source.as_raw_fd(),
            c"s".as_ptr(),
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            source.as_raw_fd(),
            c"s".as_ptr(),
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    // Unsupported atomic rename fails closed; never emulate it with check/rename.
    if result != 0 {
        return Err(INVALID);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
