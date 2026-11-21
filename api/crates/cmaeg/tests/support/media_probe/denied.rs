//! The probe's `denied` mode: every call the media job sandbox must refuse
//! with `EPERM`, each reported as `name=<errno>` (or `name=ok`).
use crate::probe::report;
use std::{ffi::CStr, ptr};

const F_SETSIG: i32 = 10;
const F_SETOWN_EX: i32 = 15;
const FIOSETOWN: u64 = 0x8901;
const SIOCSPGRP: u64 = 0x8902;
const IOPRIO_WHO_PROCESS: i32 = 1;
const IOPRIO_IDLE: i32 = 3 << 13;

fn path(value: &CStr) -> *const libc::c_char {
    value.as_ptr()
}

/// Every call the sandbox must refuse with `EPERM`.
pub(crate) fn denied(ppid: i32) {
    use libc::{syscall as sys, AT_FDCWD};
    let input = std::fs::File::open("input").expect("the job's input");
    let input_fd = std::os::fd::AsRawFd::as_raw_fd(&input);
    let siginfo = [0_u8; 128];
    let mut scratch = [0_u8; 128];
    let mut mask = [0_u8; 128];
    let local = libc::iovec {
        iov_base: scratch.as_mut_ptr().cast(),
        iov_len: 8,
    };
    let remote = libc::iovec {
        iov_base: ptr::null_mut(),
        iov_len: 8,
    };
    let owner = [0_i32, ppid];
    let mut params = [0_u8; 120];
    // SAFETY: every pointer refers to a live local buffer large enough
    // for its call; a call that unexpectedly succeeds in a child exits.
    unsafe {
        let calls: Vec<(&str, i64)> = vec![
            (
                "socket_inet",
                sys(libc::SYS_socket, libc::AF_INET, libc::SOCK_STREAM, 0),
            ),
            (
                "socket_unix",
                sys(libc::SYS_socket, libc::AF_UNIX, libc::SOCK_STREAM, 0),
            ),
            (
                "socket_x32",
                sys(
                    libc::SYS_socket | 0x4000_0000,
                    libc::AF_INET,
                    libc::SOCK_STREAM,
                    0,
                ),
            ),
            (
                "io_uring_setup",
                sys(libc::SYS_io_uring_setup, 1, params.as_mut_ptr()),
            ),
            (
                "rt_sigqueueinfo",
                sys(
                    libc::SYS_rt_sigqueueinfo,
                    ppid,
                    libc::SIGTERM,
                    siginfo.as_ptr(),
                ),
            ),
            (
                "rt_tgsigqueueinfo",
                sys(
                    libc::SYS_rt_tgsigqueueinfo,
                    ppid,
                    ppid,
                    libc::SIGTERM,
                    siginfo.as_ptr(),
                ),
            ),
            ("kill_parent", sys(libc::SYS_kill, ppid, libc::SIGTERM)),
            ("kill_group", sys(libc::SYS_kill, 0, 0)),
            ("kill_all", sys(libc::SYS_kill, -1, 0)),
            ("tkill", sys(libc::SYS_tkill, ppid, libc::SIGTERM)),
            ("tgkill", sys(libc::SYS_tgkill, ppid, ppid, libc::SIGTERM)),
            ("pidfd_open", sys(libc::SYS_pidfd_open, ppid, 0)),
            (
                "fcntl_setown",
                sys(libc::SYS_fcntl, 1, libc::F_SETOWN, ppid),
            ),
            (
                "fcntl_setown_ex",
                sys(libc::SYS_fcntl, 1, F_SETOWN_EX, owner.as_ptr()),
            ),
            (
                "fcntl_setsig",
                sys(libc::SYS_fcntl, 1, F_SETSIG, libc::SIGTERM),
            ),
            (
                "ioctl_fiosetown",
                sys(libc::SYS_ioctl, 1, FIOSETOWN, &ppid as *const i32),
            ),
            (
                "ioctl_siocspgrp",
                sys(libc::SYS_ioctl, 1, SIOCSPGRP, &ppid as *const i32),
            ),
            ("fork", child_exits(sys(libc::SYS_fork))),
            (
                "clone_process",
                child_exits(sys(libc::SYS_clone, libc::SIGCHLD, 0, 0, 0, 0)),
            ),
            (
                "prlimit_parent",
                sys(
                    libc::SYS_prlimit64,
                    ppid,
                    libc::RLIMIT_CPU,
                    0,
                    scratch.as_mut_ptr(),
                ),
            ),
            (
                "setpriority",
                sys(libc::SYS_setpriority, libc::PRIO_USER, 0, 19),
            ),
            (
                "sched_setaffinity",
                sys(
                    libc::SYS_sched_setaffinity,
                    ppid,
                    mask.len(),
                    mask.as_mut_ptr(),
                ),
            ),
            (
                "ioprio_set",
                sys(libc::SYS_ioprio_set, IOPRIO_WHO_PROCESS, ppid, IOPRIO_IDLE),
            ),
            (
                "ptrace_attach",
                sys(libc::SYS_ptrace, libc::PTRACE_ATTACH, ppid, 0, 0),
            ),
            (
                "process_vm_readv",
                sys(libc::SYS_process_vm_readv, ppid, &local, 1, &remote, 1, 0),
            ),
            ("unshare", sys(libc::SYS_unshare, 0)),
            (
                "memfd_create",
                sys(libc::SYS_memfd_create, path(c"probe"), 0),
            ),
            (
                "openat_wronly",
                sys(libc::SYS_openat, AT_FDCWD, path(c"input"), libc::O_WRONLY),
            ),
            (
                "openat_rdwr",
                sys(libc::SYS_openat, AT_FDCWD, path(c"input"), libc::O_RDWR),
            ),
            (
                "openat_creat",
                sys(
                    libc::SYS_openat,
                    AT_FDCWD,
                    path(c"created"),
                    libc::O_RDONLY | libc::O_CREAT,
                    0o600,
                ),
            ),
            (
                "openat_trunc",
                sys(
                    libc::SYS_openat,
                    AT_FDCWD,
                    path(c"input"),
                    libc::O_RDONLY | libc::O_TRUNC,
                ),
            ),
            (
                "openat_tmpfile",
                sys(
                    libc::SYS_openat,
                    AT_FDCWD,
                    path(c"."),
                    libc::O_TMPFILE | libc::O_RDWR,
                    0o600,
                ),
            ),
            (
                "open_creat",
                sys(
                    libc::SYS_open,
                    path(c"created"),
                    libc::O_RDONLY | libc::O_CREAT,
                    0o600,
                ),
            ),
            ("creat", sys(libc::SYS_creat, path(c"created"), 0o600)),
            (
                "mkdirat",
                sys(libc::SYS_mkdirat, AT_FDCWD, path(c"dir"), 0o700),
            ),
            (
                "mknodat",
                sys(
                    libc::SYS_mknodat,
                    AT_FDCWD,
                    path(c"node"),
                    libc::S_IFREG | 0o600,
                    0,
                ),
            ),
            (
                "symlinkat",
                sys(libc::SYS_symlinkat, path(c"input"), AT_FDCWD, path(c"link")),
            ),
            (
                "linkat",
                sys(
                    libc::SYS_linkat,
                    AT_FDCWD,
                    path(c"input"),
                    AT_FDCWD,
                    path(c"hard"),
                    0,
                ),
            ),
            (
                "renameat2",
                sys(
                    libc::SYS_renameat2,
                    AT_FDCWD,
                    path(c"input"),
                    AT_FDCWD,
                    path(c"moved"),
                    0,
                ),
            ),
            (
                "unlinkat",
                sys(libc::SYS_unlinkat, AT_FDCWD, path(c"input"), 0),
            ),
            ("ftruncate", sys(libc::SYS_ftruncate, input_fd, 0)),
            ("fallocate", sys(libc::SYS_fallocate, input_fd, 0, 0, 4096)),
        ];
        for (name, result) in calls {
            report(name, result);
        }
    }
}

/// A process-creating call that succeeded: the child leaves at once.
fn child_exits(result: i64) -> i64 {
    if result == 0 {
        // SAFETY: the child must not run the parent's remaining code.
        unsafe { libc::_exit(0) }
    }
    result
}
