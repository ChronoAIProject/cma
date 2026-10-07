//! The media job sandbox, on Linux x86_64 only.
//!
//! `cmaeg --media-job` prepares everything first (arguments, the program path
//! and argv, an empty environment and both BPF programs), then runs five
//! steps and executes FFmpeg or ffprobe once: close every inherited
//! descriptor above stderr, set the resource limits, switch to `SCHED_IDLE`,
//! set `no_new_privs`, and install the filters. Nothing allocates after the
//! filters are installed. Any failure fails closed: a static message on
//! stderr and exit 125, which the gateway classifies as `sandbox_unavailable`.
//!
//! `cmaeg --media-sandbox-check` runs the same five steps in its own process
//! and then proves the filters hold, so a libc gap (musl's `ENOSYS`
//! `sched_setscheduler` stub, a missing `close_range`) fails the image build
//! and startup instead of production jobs.
pub mod limits;
pub mod syscalls;

use super::argv::{self, parse_job_args};
use seccompiler::BpfProgram;
use std::{
    ffi::{CString, OsString},
    os::{raw::c_char, unix::ffi::OsStringExt},
    ptr,
};
use syscalls::{filter_a, filter_e, install_plan, Filter};

/// The exit status of a sandbox that could not be set up or held.
pub const SANDBOX_FAILED: i32 = 125;

/// A step of the wrapper that failed: 0 is argument parsing, 1 to 5 are the
/// sandbox steps, 6 is the `execve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step(pub u8);

/// One static message per step, so a failure writes without allocating.
const STEP_MESSAGES: [&[u8]; 7] = [
    b"cmaeg media-job: step 0 failed\n",
    b"cmaeg media-job: step 1 failed\n",
    b"cmaeg media-job: step 2 failed\n",
    b"cmaeg media-job: step 3 failed\n",
    b"cmaeg media-job: step 4 failed\n",
    b"cmaeg media-job: step 5 failed\n",
    b"cmaeg media-job: step 6 failed\n",
];

/// Step 1: close every descriptor above stderr. A raw syscall, because musl
/// has no `close_range` wrapper.
pub fn close_inherited() -> Result<(), Step> {
    // SAFETY: close_range takes integer arguments and touches only this
    // process's descriptor table.
    let result = unsafe { libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, 0_u32) };
    if result == 0 {
        Ok(())
    } else {
        Err(Step(1))
    }
}

/// Step 2: every limit with soft = hard.
pub fn set_limits() -> Result<(), Step> {
    for limit in limits::JOB_LIMITS {
        let value = libc::rlimit {
            rlim_cur: limit.value,
            rlim_max: limit.value,
        };
        // SAFETY: `value` is a valid rlimit for the duration of the call.
        if unsafe { libc::setrlimit(limit.resource, &value) } != 0 {
            return Err(Step(2));
        }
    }
    Ok(())
}

/// Step 3: `SCHED_IDLE`, which FFmpeg's threads inherit. A raw syscall,
/// because musl's `sched_setscheduler` is an `ENOSYS` stub.
pub fn set_idle_scheduling() -> Result<(), Step> {
    // Priority 0, the only one SCHED_IDLE takes. musl's struct has reserved
    // fields glibc's lacks, so it is zeroed rather than named.
    // SAFETY: sched_param is plain integers, for which zero is valid.
    let param: libc::sched_param = unsafe { std::mem::zeroed() };
    // SAFETY: `param` outlives the call; pid 0 is the calling thread.
    let result = unsafe {
        libc::syscall(
            libc::SYS_sched_setscheduler,
            0,
            libc::SCHED_IDLE,
            &param as *const libc::sched_param,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(Step(3))
    }
}

/// Step 4: no exec may ever gain privileges.
pub fn set_no_new_privs() -> Result<(), Step> {
    // SAFETY: PR_SET_NO_NEW_PRIVS reads only its integer arguments.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } == 0 {
        Ok(())
    } else {
        Err(Step(4))
    }
}

/// Step 5: both filters, in [`install_plan`] order.
pub fn install_filters(e: &BpfProgram, a: &BpfProgram) -> Result<(), Step> {
    for filter in install_plan() {
        let program = match filter {
            Filter::E => e,
            Filter::A => a,
        };
        seccompiler::apply_filter(program).map_err(|_| Step(5))?;
    }
    Ok(())
}

/// Steps 1 to 5, in order.
pub fn enter(e: &BpfProgram, a: &BpfProgram) -> Result<(), Step> {
    close_inherited()?;
    set_limits()?;
    set_idle_scheduling()?;
    set_no_new_privs()?;
    install_filters(e, a)
}

/// Everything the wrapper needs, built before the first step.
struct Prepared {
    path: CString,
    // Owns the strings `argv` points into.
    _args: Vec<CString>,
    argv: Vec<*const c_char>,
    envp: [*const c_char; 1],
    e: BpfProgram,
    a: BpfProgram,
}

fn prepare(args: Vec<OsString>) -> Result<Prepared, Step> {
    const PARSE: Step = Step(0);
    let invocation = parse_job_args(&args).map_err(|_| PARSE)?;
    let (name, program_args) = argv::program(&invocation.op).ok_or(PARSE)?;
    let path = CString::new(invocation.bin_dir.join(name).into_os_string().into_vec())
        .map_err(|_| PARSE)?;
    let owned = program_args
        .into_iter()
        .map(CString::new)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PARSE)?;
    let mut pointers: Vec<*const c_char> = owned.iter().map(|arg| arg.as_ptr()).collect();
    pointers.push(ptr::null());
    // `execve` keeps the pid, so the pid filters use the value taken here.
    // SAFETY: getpid has no preconditions.
    let pid = unsafe { libc::getpid() };
    Ok(Prepared {
        path,
        _args: owned,
        argv: pointers,
        envp: [ptr::null()],
        e: filter_e(),
        a: filter_a(pid),
    })
}

/// `cmaeg --media-job <op> <bin-dir> <job-dir> [arg]`: no runtime, no
/// logging and no environment read; it either executes the program or exits
/// 125.
pub fn job_main(args: Vec<OsString>) -> ! {
    let prepared = match prepare(args) {
        Ok(prepared) => prepared,
        Err(step) => fail(step),
    };
    if let Err(step) = enter(&prepared.e, &prepared.a) {
        fail(step);
    }
    // SAFETY: every pointer names a NUL-terminated string owned by
    // `prepared`, and both arrays end with a null pointer.
    unsafe {
        libc::execve(
            prepared.path.as_ptr(),
            prepared.argv.as_ptr(),
            prepared.envp.as_ptr(),
        );
    }
    fail(Step(6))
}

fn fail(step: Step) -> ! {
    let message = STEP_MESSAGES
        .get(usize::from(step.0))
        .copied()
        .unwrap_or(STEP_MESSAGES[0]);
    write_and_exit(message, SANDBOX_FAILED)
}

fn write_and_exit(message: &[u8], status: i32) -> ! {
    // SAFETY: `message` is valid for its length; `_exit` never returns.
    unsafe {
        libc::write(2, message.as_ptr().cast(), message.len());
        libc::_exit(status)
    }
}

/// One self-check: what it is called and whether it held.
type Check = (&'static [u8], fn(libc::pid_t) -> bool);

/// The self-checks, each with the static message written when it fails.
const CHECKS: [Check; 6] = [
    (
        b"cmaeg media-sandbox-check: thread failed\n",
        // `Builder::spawn` reports a refused thread instead of panicking.
        |_| {
            std::thread::Builder::new()
                .spawn(|| ())
                .is_ok_and(|thread| thread.join().is_ok())
        },
    ),
    (b"cmaeg media-sandbox-check: clone3 failed\n", |_| {
        // SAFETY: a null clone_args with size 0 is refused before any use.
        let result = unsafe { libc::syscall(libc::SYS_clone3, ptr::null::<u8>(), 0_usize) };
        result == -1 && errno() == libc::ENOSYS
    }),
    (b"cmaeg media-sandbox-check: socket failed\n", |_| {
        // SAFETY: socket takes integer arguments.
        let result = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        result == -1 && errno() == libc::EPERM
    }),
    (b"cmaeg media-sandbox-check: create failed\n", |_| {
        // SAFETY: the path is a NUL-terminated literal.
        let result = unsafe {
            libc::openat(
                libc::AT_FDCWD,
                c".".as_ptr(),
                libc::O_CREAT | libc::O_WRONLY,
                0o600,
            )
        };
        result == -1 && errno() == libc::EPERM
    }),
    (b"cmaeg media-sandbox-check: read failed\n", |_| {
        // SAFETY: the path is a NUL-terminated literal.
        let fd = unsafe {
            libc::openat(
                libc::AT_FDCWD,
                c"/proc/self/status".as_ptr(),
                libc::O_RDONLY,
            )
        };
        if fd >= 0 {
            // SAFETY: `fd` was just opened by this process.
            unsafe { libc::close(fd) };
        }
        fd >= 0
    }),
    (b"cmaeg media-sandbox-check: signal failed\n", |ppid| {
        // SAFETY: signal 0 only checks permission.
        let result = unsafe { libc::kill(ppid, 0) };
        result == -1 && errno() == libc::EPERM
    }),
];

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `cmaeg --media-sandbox-check`: enter the sandbox in this process, then
/// prove that threads still work, `clone3` falls back, and sockets, file
/// creation and signals to the parent are refused while reads are not.
pub fn sandbox_check_main() -> ! {
    // SAFETY: getppid and getpid have no preconditions.
    let (ppid, pid) = unsafe { (libc::getppid(), libc::getpid()) };
    let (e, a) = (filter_e(), filter_a(pid));
    if let Err(step) = enter(&e, &a) {
        let message: &[u8] = match step.0 {
            1 => b"cmaeg media-sandbox-check: step 1 failed\n",
            2 => b"cmaeg media-sandbox-check: step 2 failed\n",
            3 => b"cmaeg media-sandbox-check: step 3 failed\n",
            4 => b"cmaeg media-sandbox-check: step 4 failed\n",
            _ => b"cmaeg media-sandbox-check: step 5 failed\n",
        };
        write_and_exit(message, SANDBOX_FAILED);
    }
    for (message, holds) in CHECKS {
        if !holds(ppid) {
            write_and_exit(message, SANDBOX_FAILED);
        }
    }
    // SAFETY: `_exit` never returns.
    unsafe { libc::_exit(0) }
}
