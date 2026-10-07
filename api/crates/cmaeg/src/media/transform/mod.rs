//! Sandboxed FFmpeg jobs: keyframes from video and 16 kHz mono PCM from any
//! audio container.
//!
//! FFmpeg is a large C decoder that parses hostile input, so it never runs in
//! the gateway process. Each job is a fresh exec of the gateway binary that
//! limits itself, installs two seccomp filters and then executes FFmpeg or
//! ffprobe once with a fixed argv. The sandbox exists on Linux x86_64 only.
pub mod argv;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod sandbox;
