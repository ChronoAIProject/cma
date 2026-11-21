//! The media job's two seccomp filters and the syscalls neither may allow.
//!
//! **Filter E** answers `clone3` with `ENOSYS` and allows everything else.
//! **Filter A** is default-deny: a syscall outside its allow table fails with
//! `EPERM`. Process-directed calls are allowed only towards the job itself,
//! because the kernel checks nothing but the uid for signals, rlimits and
//! scheduling, and the gateway shares the job's uid. Syscall numbers outside
//! the x86_64 table (x32's `nr | 0x40000000` included) never match, and
//! seccompiler's prologue kills any other `AUDIT_ARCH`, which covers i386
//! `int 0x80`.
//!
//! The allow table is data: a later finding may add a syscall that is not on
//! [`NEVER`], with its justification.
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch,
};
use std::collections::BTreeMap;

/// `__O_TMPFILE`: `O_TMPFILE` without its `O_DIRECTORY` bit, which libc folds
/// into its own constant.
pub const O_TMPFILE_BIT: u64 = 0o20_000_000;
/// The open-flag bits that must all be clear. `O_TRUNC` is included because
/// Linux truncates on `O_RDONLY | O_TRUNC` when the file is writable.
pub const OPEN_MASK: u64 = (libc::O_ACCMODE | libc::O_CREAT | libc::O_TRUNC) as u64 | O_TMPFILE_BIT;
/// `asm/prctl.h`, which libc does not export.
const ARCH_SET_FS: u64 = 0x1002;
const ARCH_GET_FS: u64 = 0x1003;
/// `asm-generic/ioctls.h`. libc types these `c_ulong` on glibc and `c_int` on
/// musl, so they are restated here once and checked against libc in a test.
const TCGETS: u64 = 0x5401;
const TIOCGWINSZ: u64 = 0x5413;

/// A filter, in the order [`install_plan`] installs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    E,
    A,
}

/// Filter A denies `prctl` and `seccomp`, which seccompiler's own install
/// needs, so A must come last. The kernel runs every filter and keeps the
/// lowest action, so E's `ERRNO(ENOSYS)` for `clone3` overrides A's allow
/// entry; glibc falls back to `clone` only on `ENOSYS`.
pub const fn install_plan() -> [Filter; 2] {
    [Filter::E, Filter::A]
}

/// When an allowed syscall is allowed. Every comparison is on the low 32 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Allow {
    /// Whatever its arguments.
    Always,
    /// When `arg & mask == 0`.
    MaskedClear { arg: u8, mask: u64 },
    /// When `arg & mask == mask`.
    MaskedSet { arg: u8, mask: u64 },
    /// When `arg` is one of `values`.
    OneOf { arg: u8, values: &'static [u64] },
    /// When argument 0 is the job's own pid.
    OwnPid,
    /// When argument 0 is 0 (the caller) or the job's own pid.
    SelfOrOwnPid,
}

/// One allow-table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub nr: i64,
    pub allow: Allow,
    pub why: &'static str,
}

const fn always(nr: i64, why: &'static str) -> Entry {
    Entry {
        nr,
        allow: Allow::Always,
        why,
    }
}

const fn when(nr: i64, allow: Allow, why: &'static str) -> Entry {
    Entry { nr, allow, why }
}

/// Filter A's allow table.
pub const ALLOW: &[Entry] = &[
    always(libc::SYS_read, "read the input and pipes"),
    always(
        libc::SYS_write,
        "write the output to stdout and errors to stderr",
    ),
    always(libc::SYS_readv, "vectored reads by libc"),
    always(libc::SYS_writev, "vectored writes by libc"),
    always(libc::SYS_pread64, "demuxers read the input at offsets"),
    always(
        libc::SYS_pwrite64,
        "positioned writes to an already open descriptor",
    ),
    always(libc::SYS_lseek, "demuxers seek within the input"),
    always(libc::SYS_close, "close descriptors"),
    always(libc::SYS_fstat, "size of an open descriptor"),
    always(libc::SYS_newfstatat, "stat of the input and of fds"),
    always(libc::SYS_statx, "stat by newer libcs"),
    always(libc::SYS_faccessat, "access checks by libc"),
    always(libc::SYS_faccessat2, "access checks by newer libcs"),
    always(
        libc::SYS_readlinkat,
        "the runtime resolves its own executable",
    ),
    always(
        libc::SYS_getdents64,
        "directory listing of the job's own cwd",
    ),
    always(libc::SYS_getcwd, "path resolution relative to the job dir"),
    always(libc::SYS_mmap, "memory allocation, bounded by RLIMIT_AS"),
    always(libc::SYS_munmap, "memory release"),
    always(libc::SYS_mprotect, "guard pages and loader protections"),
    always(libc::SYS_mremap, "allocator growth"),
    always(libc::SYS_madvise, "allocator hints"),
    always(libc::SYS_brk, "heap growth, bounded by RLIMIT_AS"),
    always(libc::SYS_futex, "thread synchronization"),
    always(libc::SYS_set_robust_list, "thread start-up"),
    always(libc::SYS_rseq, "thread start-up by newer libcs"),
    always(libc::SYS_set_tid_address, "thread start-up"),
    always(libc::SYS_rt_sigaction, "FFmpeg installs signal handlers"),
    always(libc::SYS_rt_sigprocmask, "signal masks of threads"),
    always(libc::SYS_rt_sigreturn, "return from a signal handler"),
    always(libc::SYS_sigaltstack, "alternate signal stacks of threads"),
    always(libc::SYS_clock_gettime, "timestamps"),
    always(libc::SYS_clock_nanosleep, "sleeps"),
    always(libc::SYS_nanosleep, "sleeps"),
    always(libc::SYS_sched_yield, "thread scheduling hints"),
    always(libc::SYS_getpid, "own process id"),
    always(libc::SYS_gettid, "own thread id"),
    always(libc::SYS_getuid, "own identity"),
    always(libc::SYS_geteuid, "own identity"),
    always(libc::SYS_getgid, "own identity"),
    always(libc::SYS_getegid, "own identity"),
    always(libc::SYS_getrandom, "hash seeds and dithering"),
    always(libc::SYS_poll, "waiting on stdio"),
    always(libc::SYS_ppoll, "waiting on stdio"),
    always(libc::SYS_uname, "libc feature probes"),
    always(libc::SYS_getrlimit, "own limits"),
    always(libc::SYS_getrusage, "FFmpeg's CLI reads its own CPU time"),
    always(libc::SYS_exit, "a thread ends"),
    always(libc::SYS_exit_group, "the job ends"),
    always(libc::SYS_execve, "the wrapper executes the program once"),
    always(
        libc::SYS_clone3,
        "never reached: Filter E answers ENOSYS first",
    ),
    when(
        libc::SYS_openat,
        Allow::MaskedClear {
            arg: 2,
            mask: OPEN_MASK,
        },
        "read-only opens: no file is created, truncated or written",
    ),
    when(
        libc::SYS_open,
        Allow::MaskedClear {
            arg: 1,
            mask: OPEN_MASK,
        },
        "read-only opens by older code paths",
    ),
    when(
        libc::SYS_clone,
        Allow::MaskedSet {
            arg: 0,
            mask: libc::CLONE_THREAD as u64,
        },
        "threads only: a clone without CLONE_THREAD would be a process",
    ),
    when(libc::SYS_kill, Allow::OwnPid, "signals to the job itself"),
    when(
        libc::SYS_tkill,
        Allow::OwnPid,
        "signals to the job's own thread",
    ),
    when(
        libc::SYS_rt_sigqueueinfo,
        Allow::OwnPid,
        "queued signals to the job itself",
    ),
    when(
        libc::SYS_tgkill,
        Allow::OwnPid,
        "signals to the job's own threads",
    ),
    when(
        libc::SYS_rt_tgsigqueueinfo,
        Allow::OwnPid,
        "queued signals to the job's own threads",
    ),
    when(
        libc::SYS_prlimit64,
        Allow::SelfOrOwnPid,
        "libc reads its own limits; soft = hard, so nothing can be raised",
    ),
    when(
        libc::SYS_sched_getaffinity,
        Allow::SelfOrOwnPid,
        "FFmpeg counts its CPUs",
    ),
    when(
        libc::SYS_fcntl,
        Allow::OneOf {
            arg: 1,
            values: &[
                libc::F_DUPFD as u64,
                libc::F_GETFD as u64,
                libc::F_SETFD as u64,
                libc::F_GETFL as u64,
                libc::F_SETFL as u64,
                libc::F_DUPFD_CLOEXEC as u64,
            ],
        },
        "descriptor flags; never ownership or signal delivery",
    ),
    when(
        libc::SYS_ioctl,
        Allow::OneOf {
            arg: 1,
            values: &[TCGETS, TIOCGWINSZ],
        },
        "libc asks whether stdio is a terminal",
    ),
    when(
        libc::SYS_prctl,
        Allow::OneOf {
            arg: 0,
            values: &[libc::PR_SET_NAME as u64, libc::PR_GET_NAME as u64],
        },
        "thread names",
    ),
    when(
        libc::SYS_arch_prctl,
        Allow::OneOf {
            arg: 0,
            values: &[ARCH_SET_FS, ARCH_GET_FS],
        },
        "thread-local storage",
    ),
];

/// Syscalls the allow table must never contain, in any form. An
/// unconditional `open` or `openat` is refused by a separate check.
pub const NEVER: &[i64] = &[
    // Sockets.
    libc::SYS_socket,
    libc::SYS_socketpair,
    libc::SYS_connect,
    libc::SYS_bind,
    libc::SYS_listen,
    libc::SYS_accept,
    libc::SYS_accept4,
    libc::SYS_sendto,
    libc::SYS_sendmsg,
    libc::SYS_sendmmsg,
    libc::SYS_recvfrom,
    libc::SYS_recvmsg,
    libc::SYS_recvmmsg,
    libc::SYS_getsockopt,
    libc::SYS_setsockopt,
    libc::SYS_shutdown,
    libc::SYS_io_uring_setup,
    libc::SYS_io_uring_enter,
    libc::SYS_io_uring_register,
    // Processes and namespaces.
    libc::SYS_fork,
    libc::SYS_vfork,
    libc::SYS_unshare,
    libc::SYS_setns,
    libc::SYS_setsid,
    libc::SYS_setpgid,
    // Other processes.
    libc::SYS_ptrace,
    libc::SYS_process_vm_readv,
    libc::SYS_process_vm_writev,
    libc::SYS_process_madvise,
    libc::SYS_pidfd_open,
    libc::SYS_pidfd_send_signal,
    libc::SYS_pidfd_getfd,
    libc::SYS_kcmp,
    // Kernel interfaces.
    libc::SYS_perf_event_open,
    libc::SYS_bpf,
    libc::SYS_userfaultfd,
    libc::SYS_keyctl,
    libc::SYS_add_key,
    libc::SYS_request_key,
    // Mounts.
    libc::SYS_mount,
    libc::SYS_umount2,
    libc::SYS_fsopen,
    libc::SYS_fsmount,
    libc::SYS_fsconfig,
    libc::SYS_move_mount,
    libc::SYS_open_tree,
    libc::SYS_chroot,
    libc::SYS_pivot_root,
    // Scheduling of others.
    libc::SYS_setpriority,
    libc::SYS_sched_setscheduler,
    libc::SYS_sched_setparam,
    libc::SYS_sched_setaffinity,
    libc::SYS_sched_setattr,
    libc::SYS_ioprio_set,
    libc::SYS_memfd_create,
    libc::SYS_inotify_init1,
    libc::SYS_inotify_add_watch,
    libc::SYS_fanotify_init,
    libc::SYS_fanotify_mark,
    libc::SYS_timer_create,
    // IPC.
    libc::SYS_mq_open,
    libc::SYS_msgget,
    libc::SYS_msgsnd,
    libc::SYS_semget,
    libc::SYS_semop,
    libc::SYS_shmget,
    libc::SYS_shmat,
    libc::SYS_personality,
    libc::SYS_seccomp,
    libc::SYS_move_pages,
    libc::SYS_migrate_pages,
    libc::SYS_execveat,
    // File creation and mutation.
    libc::SYS_creat,
    libc::SYS_mknod,
    libc::SYS_mknodat,
    libc::SYS_mkdir,
    libc::SYS_mkdirat,
    libc::SYS_link,
    libc::SYS_linkat,
    libc::SYS_symlink,
    libc::SYS_symlinkat,
    libc::SYS_rename,
    libc::SYS_renameat,
    libc::SYS_renameat2,
    libc::SYS_unlink,
    libc::SYS_unlinkat,
    libc::SYS_rmdir,
    libc::SYS_truncate,
    libc::SYS_ftruncate,
    libc::SYS_fallocate,
    libc::SYS_openat2,
    libc::SYS_open_by_handle_at,
    libc::SYS_name_to_handle_at,
    // Permissions and attributes.
    libc::SYS_chmod,
    libc::SYS_fchmod,
    libc::SYS_fchmodat,
    libc::SYS_chown,
    libc::SYS_fchown,
    libc::SYS_lchown,
    libc::SYS_fchownat,
    libc::SYS_utimensat,
    libc::SYS_setxattr,
    libc::SYS_lsetxattr,
    libc::SYS_fsetxattr,
    libc::SYS_removexattr,
    libc::SYS_lremovexattr,
    libc::SYS_fremovexattr,
];

/// Filter E: `clone3` fails with `ENOSYS`, so libc falls back to `clone`,
/// which Filter A restricts to threads.
pub fn filter_e() -> BpfProgram {
    compile(
        BTreeMap::from([(libc::SYS_clone3, Vec::new())]),
        SeccompAction::Allow,
        SeccompAction::Errno(libc::ENOSYS as u32),
    )
}

/// Filter A for the job whose pid, taken before the filters are installed
/// and kept across `execve`, is `pid`.
pub fn filter_a(pid: i32) -> BpfProgram {
    let rules = ALLOW
        .iter()
        .map(|entry| (entry.nr, rules(entry.allow, pid)))
        .collect();
    compile(
        rules,
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Allow,
    )
}

fn rules(allow: Allow, pid: i32) -> Vec<SeccompRule> {
    // A pid_t compares on its 32 bits, so negative pids never equal it.
    let own = u64::from(pid as u32);
    match allow {
        Allow::Always => Vec::new(),
        Allow::MaskedClear { arg, mask } => vec![rule(arg, SeccompCmpOp::MaskedEq(mask), 0)],
        Allow::MaskedSet { arg, mask } => vec![rule(arg, SeccompCmpOp::MaskedEq(mask), mask)],
        Allow::OneOf { arg, values } => values
            .iter()
            .map(|value| rule(arg, SeccompCmpOp::Eq, *value))
            .collect(),
        Allow::OwnPid => vec![rule(0, SeccompCmpOp::Eq, own)],
        Allow::SelfOrOwnPid => vec![rule(0, SeccompCmpOp::Eq, 0), rule(0, SeccompCmpOp::Eq, own)],
    }
}

fn rule(arg: u8, op: SeccompCmpOp, value: u64) -> SeccompRule {
    let condition = SeccompCondition::new(arg, SeccompCmpArgLen::Dword, op, value)
        .expect("argument indexes in the allow table are below 6");
    SeccompRule::new(vec![condition]).expect("every rule has one condition")
}

/// The tables are fixed, so a compile failure is a defect the
/// `filters_compile` test catches, never an input-dependent condition.
fn compile(
    rules: BTreeMap<i64, Vec<SeccompRule>>,
    mismatch: SeccompAction,
    matched: SeccompAction,
) -> BpfProgram {
    let filter = SeccompFilter::new(rules, mismatch, matched, TargetArch::x86_64)
        .expect("the match and mismatch actions differ");
    BpfProgram::try_from(filter).expect("the filter fits a BPF program")
}

#[cfg(test)]
mod tests;
