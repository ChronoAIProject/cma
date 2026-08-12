//! Claude Code implementation of [`crate::KernelAdapter`].

use crate::{AdapterError, KernelAdapter, TermSize};

/// Drives a Claude Code runtime.
#[derive(Debug, Default)]
pub struct ClaudeCodeAdapter;

#[async_trait::async_trait]
impl KernelAdapter for ClaudeCodeAdapter {
    fn kernel_name(&self) -> &'static str {
        "claude-code"
    }

    async fn start_turn(&self, _prompt: &str) -> Result<(), AdapterError> {
        Err(AdapterError::Unavailable)
    }

    async fn interrupt(&self) -> Result<(), AdapterError> {
        Err(AdapterError::Unavailable)
    }

    async fn resize(&self, _size: TermSize) -> Result<(), AdapterError> {
        Err(AdapterError::Unavailable)
    }
}
