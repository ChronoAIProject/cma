use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd as _, FromRawFd as _};
use std::os::unix::fs::FileExt as _;

pub(in crate::desktop_process) fn create() -> io::Result<File> {
    let fd = unsafe {
        libc::memfd_create(
            c"cma-desktop-token".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

pub(super) fn seal(file: &File, token: &str, destination: &str) -> io::Result<()> {
    let mapping = format!("{token}: unix_socket:{destination}\n");
    file.set_len(0)?;
    file.write_all_at(mapping.as_bytes(), 0)?;
    let seals = libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_ADD_SEALS, seals) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(in crate::desktop_process) fn endpoint(file: &File) -> String {
    format!("/proc/{}/fd/{}", std::process::id(), file.as_raw_fd())
}
