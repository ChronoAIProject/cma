//! The media job sandbox, on Linux x86_64 only: the resource limits and the
//! seccomp filters a job runs under.
pub mod limits;
pub mod syscalls;
