//! Layer 3 — ACP-shaped kernel adapters.
//!
//! This file is the abstraction. It must contain no kernel-specific type,
//! constant, or conditional. Adding a kernel means adding a module, never
//! editing this trait to accommodate it.

pub mod claude_code;
pub mod codex;

/// Size of the runtime's terminal, in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// Errors an adapter may surface.
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// The runtime could not be provisioned.
    #[error("runtime unavailable")]
    Unavailable,
    /// The runtime rejected the request.
    #[error("runtime rejected request: {0}")]
    Rejected(String),
}

/// A kernel runtime, abstracted. Layer 2 depends on this trait and never on an
/// implementation of it.
#[async_trait::async_trait]
pub trait KernelAdapter: Send + Sync {
    /// Human-readable kernel name, for diagnostics only.
    fn kernel_name(&self) -> &'static str;

    /// Start a prompt turn.
    async fn start_turn(&self, prompt: &str) -> Result<(), AdapterError>;

    /// Interrupt the in-flight turn, if any.
    async fn interrupt(&self) -> Result<(), AdapterError>;

    /// Inform the runtime that its terminal was resized.
    async fn resize(&self, size: TermSize) -> Result<(), AdapterError>;
}
