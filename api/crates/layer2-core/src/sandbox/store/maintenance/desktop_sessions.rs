//! Shared desktop admission before opening any provider connection.

use crate::{sandbox::RuntimeBinding, SandboxId};

#[derive(Clone)]
pub struct DesktopSessionRequest {
    pub sandbox_id: SandboxId,
    pub session_id: String,
    /// Exact authorized binding: a delayed open cannot attach to a replacement.
    pub runtime: RuntimeBinding,
}
