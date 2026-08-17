//! Names the bot pod's supervisor reads, stated once.
//!
//! The supervisor and this crate are separate binaries that never link, so
//! anything they agree on is agreed by spelling it identically in two places.
//! `supervisor` holds the wire they speak over a socket; this holds the
//! environment an adapter starts one with.
//!
//! Only the names that are a *contract between the two* belong here. The rest
//! of a sandbox's environment is the kernel's own vocabulary and lives in that
//! kernel's directory, where nothing else can read it by accident.

/// Where a kernel's configuration file goes, relative to the pod's home.
///
/// Absent means this kernel needs no file written, which is true of a kernel
/// configured entirely by environment — and the supervisor writes nothing
/// rather than one kernel's file into every pod.
pub const CONFIG_PATH_VAR: &str = "CHRONO_RUNTIME_CONFIG_PATH";

/// That file's content, verbatim.
///
/// Composed by the adapter that owns the kernel, because it is that kernel's
/// own format. The supervisor writes it down and reads none of it.
pub const CONFIG_BODY_VAR: &str = "CHRONO_RUNTIME_CONFIG";
