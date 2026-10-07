//! Filter A's table rules and both filters' construction.
use super::*;

#[test]
fn install_plan_is_e_then_a() {
    assert_eq!(install_plan(), [Filter::E, Filter::A]);
}

#[test]
fn clone3_is_allowed_only_because_e_comes_first() {
    assert!(ALLOW.iter().any(|entry| entry.nr == libc::SYS_clone3));
    assert_eq!(install_plan()[0], Filter::E);
}

#[test]
fn never_list_is_disjoint_from_the_allow_table() {
    for entry in ALLOW {
        assert!(
            !NEVER.contains(&entry.nr),
            "syscall {} ({:?}) is on the never-list",
            entry.nr,
            entry.allow
        );
    }
}

#[test]
fn open_and_openat_are_only_argument_filtered() {
    for nr in [libc::SYS_open, libc::SYS_openat] {
        let entries: Vec<_> = ALLOW.iter().filter(|entry| entry.nr == nr).collect();
        assert_eq!(entries.len(), 1);
        assert!(matches!(
            entries[0].allow,
            Allow::MaskedClear {
                mask: OPEN_MASK,
                ..
            }
        ));
    }
    assert_eq!(
        OPEN_MASK & (libc::O_WRONLY | libc::O_RDWR | libc::O_CREAT | libc::O_TRUNC) as u64,
        (libc::O_WRONLY | libc::O_RDWR | libc::O_CREAT | libc::O_TRUNC) as u64
    );
    assert_ne!(OPEN_MASK & libc::O_TMPFILE as u64, 0);
}

/// Every syscall the pinned static FFmpeg made in probe P8's strace, running
/// each op's argv on every committed fixture.
const P8_RECORD: [i64; 25] = [
    libc::SYS_arch_prctl,
    libc::SYS_brk,
    libc::SYS_clock_gettime,
    libc::SYS_clone,
    libc::SYS_close,
    libc::SYS_exit,
    libc::SYS_exit_group,
    libc::SYS_fcntl,
    libc::SYS_fstat,
    libc::SYS_futex,
    libc::SYS_getrusage,
    libc::SYS_ioctl,
    libc::SYS_lseek,
    libc::SYS_mmap,
    libc::SYS_mprotect,
    libc::SYS_munmap,
    libc::SYS_open,
    libc::SYS_prctl,
    libc::SYS_read,
    libc::SYS_rt_sigaction,
    libc::SYS_rt_sigprocmask,
    libc::SYS_sched_getaffinity,
    libc::SYS_set_tid_address,
    libc::SYS_write,
    libc::SYS_writev,
];

#[test]
fn every_entry_has_a_justification() {
    assert!(ALLOW.iter().all(|entry| !entry.why.trim().is_empty()));
    // The pinned FFmpeg needs nothing outside the table.
    for nr in P8_RECORD {
        assert!(
            ALLOW.iter().any(|entry| entry.nr == nr),
            "syscall {nr} of the P8 record is not allowed"
        );
    }
    let mut numbers: Vec<i64> = ALLOW.iter().map(|entry| entry.nr).collect();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), ALLOW.len(), "a syscall is listed twice");
}

#[test]
fn restated_constants_match_libc() {
    assert_eq!(TCGETS as i64, libc::TCGETS as i64);
    assert_eq!(TIOCGWINSZ as i64, libc::TIOCGWINSZ as i64);
}

#[test]
fn filters_compile() {
    assert!(!filter_e().is_empty());
    assert!(!filter_a(4242).is_empty());
}
