//! A stand-in for `ffmpeg` and `ffprobe` that the media sandbox tests execute
//! through `cmaeg --media-job`. It never ships: the image builds the `cmaeg`
//! binary only.
//!
//! Its personality is argv[0]'s basename and its mode is the first line of
//! `./input`, so a test controls it through the job directory alone. Every
//! mode reports `name=value` lines on stdout; `stdout-flood <bytes>` writes
//! that many bytes and exits 0, and without a count it writes until stdout
//! fails. The parent's pid comes from `/proc/self/status`, because the
//! sandbox refuses `getppid`. The `denied` mode lives in its own module, as
//! does `media`, which writes well-formed output for whichever op runs it;
//! `broken` exits 1.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn main() {
    probe::main()
}

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
fn main() {
    std::process::exit(125)
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "media_probe/denied.rs"]
mod denied;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "media_probe/media.rs"]
mod media;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod probe {
    use std::{
        io::Write,
        ptr,
        time::{Duration, Instant},
    };

    pub fn main() {
        let input = std::fs::read_to_string("input").unwrap_or_default();
        let mode = input.lines().next().unwrap_or_default().to_owned();
        let ppid = parent_pid();
        match mode.split_whitespace().collect::<Vec<_>>().as_slice() {
            ["denied"] => crate::denied::denied(ppid),
            ["media"] => crate::media::media(&program_name()),
            ["broken"] => std::process::exit(1),
            ["clone3"] => report(
                "clone3",
                // SAFETY: a null clone_args with size 0 is refused before use.
                unsafe { libc::syscall(libc::SYS_clone3, ptr::null::<u8>(), 0_usize) },
            ),
            ["allowed"] => allowed(&input),
            ["inode-flood"] => inode_flood(),
            ["env"] => environment(),
            ["sleep"] => {
                // The launcher's tests find the job's process group by it.
                println!("pid={}", std::process::id());
                std::thread::sleep(Duration::from_secs(60));
            }
            ["spin"] => spin(),
            ["pte-flood"] => pte_flood(),
            ["thread-flood"] => thread_flood(),
            ["stdout-flood"] => stdout_flood(u64::MAX),
            ["stdout-flood", bytes] => stdout_flood(bytes.parse().unwrap_or(0)),
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
    pub(crate) fn report(name: &str, result: i64) {
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
        println!("program={}", program_name());
    }

    /// argv[0]'s basename: `ffmpeg` or `ffprobe`.
    fn program_name() -> String {
        let program = std::env::args_os().next().unwrap_or_default();
        std::path::Path::new(&program)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned()
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

    /// Busy loops on four threads, so the CPU limit is spent well before
    /// the per-process wall clock.
    fn spin() {
        let busy = || {
            let started = Instant::now();
            let mut value = 0_u64;
            while started.elapsed() < Duration::from_secs(120) {
                value = std::hint::black_box(value.wrapping_add(1));
            }
        };
        let threads: Vec<_> = (0..3)
            .filter_map(|_| std::thread::Builder::new().spawn(busy).ok())
            .collect();
        busy();
        for thread in threads {
            let _ = thread.join();
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

    /// `limit` bytes of output, then exit 0; `u64::MAX` writes until stdout
    /// fails.
    fn stdout_flood(limit: u64) {
        let chunk = vec![b'o'; 64 * 1024];
        let mut stdout = std::io::stdout().lock();
        let mut left = limit;
        while left > 0 {
            let size = usize::try_from(left.min(chunk.len() as u64)).unwrap_or(chunk.len());
            if stdout.write_all(&chunk[..size]).is_err() {
                return;
            }
            left -= size as u64;
        }
        let _ = stdout.flush();
    }
}
