//! The media job's resource limits.
//!
//! Every limit sets soft = hard: `prlimit64` on the job itself is allowed, and
//! could otherwise raise a soft limit up to its hard one. `RLIMIT_NPROC` is
//! deliberately absent: it counts processes per uid across the whole node,
//! and uid 10001 is shared with other workloads. Seccomp refuses process
//! creation instead.

/// The resource argument of `setrlimit`, which glibc and musl declare with
/// different integer types.
#[cfg(target_env = "gnu")]
pub type Resource = libc::__rlimit_resource_t;
#[cfg(not(target_env = "gnu"))]
pub type Resource = libc::c_int;

/// One resource limit, applied with soft = hard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobLimit {
    pub name: &'static str,
    pub resource: Resource,
    pub value: u64,
    pub why: &'static str,
}

/// The limits, in the order the wrapper sets them.
pub const JOB_LIMITS: [JobLimit; 7] = [
    JobLimit {
        name: "RLIMIT_AS",
        resource: libc::RLIMIT_AS,
        value: 402_653_184,
        why: "384 MiB of address space: one job's term in the memory gate",
    },
    JobLimit {
        name: "RLIMIT_CPU",
        resource: libc::RLIMIT_CPU,
        value: 20,
        why: "soft = hard, so the kernel sends SIGKILL, never SIGXCPU",
    },
    JobLimit {
        name: "RLIMIT_FSIZE",
        resource: libc::RLIMIT_FSIZE,
        value: 0,
        why: "the job writes no file",
    },
    JobLimit {
        name: "RLIMIT_NOFILE",
        resource: libc::RLIMIT_NOFILE,
        value: 64,
        why: "one input and FFmpeg's own handles need far fewer descriptors",
    },
    JobLimit {
        name: "RLIMIT_CORE",
        resource: libc::RLIMIT_CORE,
        value: 0,
        why: "a crash on hostile input never leaves a core file",
    },
    JobLimit {
        name: "RLIMIT_MEMLOCK",
        resource: libc::RLIMIT_MEMLOCK,
        value: 0,
        why: "the job never pins memory",
    },
    JobLimit {
        name: "RLIMIT_SIGPENDING",
        resource: libc::RLIMIT_SIGPENDING,
        value: 64,
        why: "bounds the job's own memcg-charged queue of real-time signals",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_limit_is_distinct_and_justified() {
        let mut resources: Vec<Resource> = JOB_LIMITS.iter().map(|limit| limit.resource).collect();
        resources.sort_unstable();
        resources.dedup();
        assert_eq!(resources.len(), JOB_LIMITS.len());
        assert!(JOB_LIMITS.iter().all(|limit| !limit.why.trim().is_empty()));
        assert!(!JOB_LIMITS
            .iter()
            .any(|limit| limit.resource == libc::RLIMIT_NPROC));
    }
}
