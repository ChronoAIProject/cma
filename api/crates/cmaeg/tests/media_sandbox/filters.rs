//! What the filters refuse and allow, the job's environment and limits, and
//! how the wrapper fails closed.
use super::support::{
    bin_dir, count_sigterms, job_dir, report, run_job, run_job_in, run_mode, sigterms, EPERM,
};
use cmaeg::media::transform::sandbox::limits::JOB_LIMITS;
use std::ffi::OsStr;

const DENIED: [&str; 41] = [
    "socket_inet",
    "socket_unix",
    "socket_x32",
    "io_uring_setup",
    "rt_sigqueueinfo",
    "rt_tgsigqueueinfo",
    "kill_parent",
    "kill_group",
    "kill_all",
    "tkill",
    "tgkill",
    "pidfd_open",
    "fcntl_setown",
    "fcntl_setown_ex",
    "fcntl_setsig",
    "ioctl_fiosetown",
    "ioctl_siocspgrp",
    "fork",
    "clone_process",
    "prlimit_parent",
    "setpriority",
    "sched_setaffinity",
    "ioprio_set",
    "ptrace_attach",
    "process_vm_readv",
    "unshare",
    "memfd_create",
    "openat_wronly",
    "openat_rdwr",
    "openat_creat",
    "openat_trunc",
    "openat_tmpfile",
    "open_creat",
    "creat",
    "mkdirat",
    "mknodat",
    "symlinkat",
    "linkat",
    "renameat2",
    "unlinkat",
    "ftruncate",
];

fn skip_as_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    let root = unsafe { libc::geteuid() } == 0;
    if root {
        eprintln!("skipped: root bypasses the permission checks this test relies on");
    }
    root
}

#[test]
fn denied_calls_return_eperm() {
    if skip_as_root() {
        return;
    }
    count_sigterms();
    let output = run_mode("denied");
    assert!(output.status.success(), "{output:?}");
    let report = report(&output);
    for name in DENIED.iter().chain(&["fallocate"]) {
        assert_eq!(
            report.get(*name).map(String::as_str),
            Some(EPERM),
            "{name}: {report:?}"
        );
    }
    assert_eq!(sigterms(), 0, "a signal reached the parent");
}

#[test]
fn clone3_returns_enosys() {
    let report = report(&run_mode("clone3"));
    assert_eq!(report["clone3"], libc::ENOSYS.to_string());
}

#[test]
fn allowed_calls_succeed() {
    let output = run_mode("allowed");
    assert!(output.status.success(), "{output:?}");
    let report = report(&output);
    assert_eq!(report["thread"], "ok");
    assert_eq!(report["kill_self"], "ok");
    assert_eq!(report["tgkill_self"], "ok");
    assert_eq!(report["read"], "8");
    assert_eq!(report["program"], "ffprobe");
}

#[test]
fn inode_flood_first_create_is_eperm() {
    let report = report(&run_mode("inode-flood"));
    assert_eq!(report["inode_flood"], EPERM);
    assert_eq!(report["created"], "0");
}

#[test]
fn job_environment_is_empty_and_limited() {
    let report = report(&run_mode("env"));
    assert_eq!(report["env"], "0");
    for limit in JOB_LIMITS {
        assert_eq!(
            report[limit.name],
            format!("{0}/{0}", limit.value),
            "{}",
            limit.name
        );
    }
    assert_eq!(report["policy"], libc::SCHED_IDLE.to_string());
    // 0, 1 and 2, and the descriptor the probe lists /proc/self/fd through.
    assert_eq!(report["fds"], "0,1,2,3");
}

#[test]
fn invalid_arguments_exit_125_without_exec() {
    use std::os::unix::ffi::OsStrExt;
    let bin = bin_dir();
    let job = job_dir("allowed");
    let bad_time = run_job("frame", bin.path(), job.path(), Some(OsStr::new("1.5")));
    // Neither invalid job-dir argument names a directory, so both run from
    // the real job directory and only the argument is wrong.
    let relative = run_job_in(
        job.path(),
        "probe",
        bin.path(),
        "relative/job".as_ref(),
        None,
    );
    let mut non_utf8 = job.path().as_os_str().as_bytes().to_vec();
    non_utf8.push(0xff);
    let non_utf8 = run_job_in(
        job.path(),
        "probe",
        bin.path(),
        OsStr::from_bytes(&non_utf8).as_ref(),
        None,
    );
    for output in [bad_time, relative, non_utf8] {
        assert_eq!(output.status.code(), Some(125), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert_eq!(output.stderr, b"cmaeg media-job: step 0 failed\n");
    }
}

#[test]
fn sandbox_check_passes_on_glibc() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_cmaeg"))
        .arg("--media-sandbox-check")
        .env_clear()
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}
