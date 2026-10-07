//! Sandboxed FFmpeg jobs: keyframes from video and 16 kHz mono PCM from any
//! audio container.
//!
//! FFmpeg is a large C decoder that parses hostile input, so it never runs in
//! the gateway process. Each job is a fresh exec of the gateway binary that
//! limits itself, installs two seccomp filters and then executes FFmpeg or
//! ffprobe once with a fixed argv. The sandbox exists on Linux x86_64 only;
//! elsewhere both job modes exit 125 without executing anything.
pub mod argv;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod sandbox;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub use sandbox::{job_main, sandbox_check_main};

/// The exit status of a job whose sandbox is unavailable.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
const SANDBOX_UNAVAILABLE: i32 = 125;

/// `cmaeg --media-job` where no sandbox exists: exit 125 without executing.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn job_main(_args: Vec<std::ffi::OsString>) -> ! {
    eprintln!("cmaeg media-job: the sandbox requires linux x86_64");
    std::process::exit(SANDBOX_UNAVAILABLE)
}

/// `cmaeg --media-sandbox-check` where no sandbox exists: always fails.
#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub fn sandbox_check_main() -> ! {
    eprintln!("cmaeg media-sandbox-check: the sandbox requires linux x86_64");
    std::process::exit(SANDBOX_UNAVAILABLE)
}
