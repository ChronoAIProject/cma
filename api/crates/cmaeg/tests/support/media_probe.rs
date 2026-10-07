//! A stand-in for `ffmpeg` and `ffprobe` that the media sandbox tests execute
//! through `cmaeg --media-job`. It never ships: the image builds the `cmaeg`
//! binary only.
//!
//! Its personality is argv[0]'s basename and its mode is the first line of
//! `./input`, so a test controls it through the job directory alone. Every
//! mode reports `name=value` lines on stdout. The parent's pid comes from
//! `/proc/self/status`, because the sandbox refuses `getppid`.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn main() {
    probe::main()
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn main() {
    std::process::exit(125)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod probe {
    use std::{
        ffi::CStr,
        io::Write,
        ptr,
        time::{Duration, Instant},
    };

    const F_SETSIG: i32 = 10;
    const F_SETOWN_EX: i32 = 15;
    const FIOSETOWN: u64 = 0x8901;
    const SIOCSPGRP: u64 = 0x8902;
    const IOPRIO_WHO_PROCESS: i32 = 1;
    const IOPRIO_IDLE: i32 = 3 << 13;

    pub fn main() {
        let input = std::fs::read_to_string("input").unwrap_or_default();
        let mode = input.lines().next().unwrap_or_default().to_owned();
        let ppid = parent_pid();
        match mode.split_whitespace().collect::<Vec<_>>().as_slice() {
            ["denied"] => denied(ppid),
            ["clone3"] => report(
                "clone3",
                // SAFETY: a null clone_args with size 0 is refused before use.
                unsafe { libc::syscall(libc::SYS_clone3, ptr::null::<u8>(), 0_usize) },
            ),
            ["allowed"] => allowed(&input),
            ["inode-flood"] => inode_flood(),
            ["env"] => environment(),
            ["sleep"] => std::thread::sleep(Duration::from_secs(60)),
            ["spin"] => spin(),
            ["pte-flood"] => pte_flood(),
            ["thread-flood"] => thread_flood(),
            ["stdout-flood"] => stdout_flood(),
            ["stderr-flood"] => {
                let _ = std::io::stderr().write_all(&vec![b'e'; 1 << 20]);
            }
            ["exit", code] => std::process::exit(code.parse().unwrap_or(1)),
            _ => std::process::exit(2),
        }
    }

    fn errno() -> i32 {
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }

    /// `name=<errno>` for a failed call, `name=ok` otherwise.
    fn report(name: &str, result: i64) {
        if result == -1 {
            println!("{name}={}", errno());
        } else {
            println!("{name}=ok");
        }
    }

    fn parent_pid() -> i32 {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status
                    .lines()
                    .find_map(|line| line.strip_prefix("PPid:"))
                    .and_then(|value| value.trim().parse().ok())
            })
            .unwrap_or(0)
    }

    fn path(value: &CStr) -> *const libc::c_char {
        value.as_ptr()
    }

    /// Every call the sandbox must refuse with `EPERM`.
    fn denied(ppid: i32) {
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

    fn allowed(input: &str) {
        let thread = std::thread::Builder::new()
            .spawn(|| 7)
            .map(|handle| handle.join().unwrap_or(0));
        println!(
            "thread={}",
            if matches!(thread, Ok(7)) {
                "ok"
            } else {
                "failed"
            }
        );
        // SAFETY: signal 0 to this process and its main thread checks only.
        let (kill, tgkill) = unsafe {
            let pid = libc::getpid();
            (
                libc::syscall(libc::SYS_kill, pid, 0),
                libc::syscall(libc::SYS_tgkill, pid, pid, 0),
            )
        };
        report("kill_self", kill);
        report("tgkill_self", tgkill);
        println!("read={}", input.len());
        let program = std::env::args_os().next().unwrap_or_default();
        let name = std::path::Path::new(&program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        println!("program={name}");
    }

    fn inode_flood() {
        for index in 0..100_000_u32 {
            let name = std::ffi::CString::new(format!("flood-{index}")).unwrap_or_default();
            // SAFETY: `name` is NUL-terminated and outlives the call.
            let fd = unsafe {
                libc::openat(
                    libc::AT_FDCWD,
                    name.as_ptr(),
                    libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY,
                    0o600,
                )
            };
            if fd < 0 {
                println!("inode_flood={}", errno());
                println!("created={index}");
                return;
            }
        }
        println!("inode_flood=none");
    }

    fn environment() {
        println!("env={}", std::env::vars_os().count());
        for (name, resource) in [
            ("RLIMIT_AS", libc::RLIMIT_AS),
            ("RLIMIT_CPU", libc::RLIMIT_CPU),
            ("RLIMIT_FSIZE", libc::RLIMIT_FSIZE),
            ("RLIMIT_NOFILE", libc::RLIMIT_NOFILE),
            ("RLIMIT_CORE", libc::RLIMIT_CORE),
            ("RLIMIT_MEMLOCK", libc::RLIMIT_MEMLOCK),
            ("RLIMIT_SIGPENDING", libc::RLIMIT_SIGPENDING),
        ] {
            let mut limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            // SAFETY: `limit` is a valid out-parameter.
            unsafe { libc::getrlimit(resource, &mut limit) };
            println!("{name}={}/{}", limit.rlim_cur, limit.rlim_max);
        }
        // Field 41 of /proc/self/stat, counted after the parenthesised name.
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
        let policy = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(38))
            .unwrap_or("unknown")
            .to_owned();
        println!("policy={policy}");
        let mut fds: Vec<u32> = std::fs::read_dir("/proc/self/fd")
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
                    .collect()
            })
            .unwrap_or_default();
        fds.sort_unstable();
        let fds: Vec<String> = fds.iter().map(u32::to_string).collect();
        println!("fds={}", fds.join(","));
    }

    fn spin() {
        let started = Instant::now();
        let mut value = 0_u64;
        while started.elapsed() < Duration::from_secs(120) {
            value = std::hint::black_box(value.wrapping_add(1));
        }
    }

    /// One page per gigabyte of address space: each mapping costs a page of
    /// RLIMIT_AS but its own page tables, which only the cgroup counts.
    fn pte_flood() {
        let mut mapped = 0_u64;
        for index in 1..u64::MAX {
            // SAFETY: an anonymous private mapping at a hint address; the
            // kernel picks another address if the hint is taken.
            let address = unsafe {
                libc::mmap(
                    (index << 30) as *mut libc::c_void,
                    4096,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if address == libc::MAP_FAILED {
                break;
            }
            // SAFETY: the page was just mapped writable.
            unsafe { address.cast::<u8>().write_volatile(1) };
            mapped += 1;
        }
        println!("mapped={mapped}");
        std::thread::sleep(Duration::from_secs(60));
    }

    fn thread_flood() {
        let mut threads = 0_u64;
        while std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(|| std::thread::sleep(Duration::from_secs(60)))
            .is_ok()
        {
            threads += 1;
        }
        println!("threads={threads}");
        std::thread::sleep(Duration::from_secs(60));
    }

    fn stdout_flood() {
        let chunk = vec![b'o'; 64 * 1024];
        let mut stdout = std::io::stdout().lock();
        while stdout.write_all(&chunk).is_ok() {}
    }
}
