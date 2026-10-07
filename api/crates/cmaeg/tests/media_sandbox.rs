//! The media job sandbox, exercised for real: the gateway binary runs the
//! probe binary as `ffmpeg` and `ffprobe` under its limits and filters.
//! Linux x86_64 only, like the sandbox itself, and meaningful as non-root.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

#[path = "media_sandbox/filters.rs"]
mod filters;
#[path = "media_sandbox/lifecycle.rs"]
mod lifecycle;
#[path = "media_sandbox/support.rs"]
mod support;
