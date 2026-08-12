//! Codex implementation of [`crate::KernelAdapter`].

use crate::{AdapterError, KernelAdapter, TermSize};

/// Drives an OpenAI Codex runtime.
#[derive(Debug, Default)]
pub struct CodexAdapter;

#[async_trait::async_trait]
impl KernelAdapter for CodexAdapter {
    fn kernel_name(&self) -> &'static str {
        "codex"
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
