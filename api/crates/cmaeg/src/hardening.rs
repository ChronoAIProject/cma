//! Process hardening applied before any mode runs.
use std::io;

/// Make this process non-dumpable.
///
/// A same-uid process can read a dumpable process's `/proc/<pid>/environ`,
/// which holds the master keys and service tokens, and its `/proc/<pid>/mem`,
/// and can ptrace it. The files under a non-dumpable process's `/proc/<pid>`
/// are owned by root instead (the directory itself keeps the process's uid),
/// and it never writes a core dump.
#[cfg(target_os = "linux")]
pub fn disable_dumping() -> io::Result<()> {
    // SAFETY: PR_SET_DUMPABLE reads only its integer arguments.
    let result = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Development builds on other platforms have no equivalent; production runs
/// only on Linux.
#[cfg(not(target_os = "linux"))]
pub fn disable_dumping() -> io::Result<()> {
    Ok(())
}
