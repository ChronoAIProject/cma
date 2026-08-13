//! Layer 3 — ACP-shaped kernel adapters.
//!
//! This file is the abstraction. It must contain no kernel-specific type,
//! constant, or conditional. Adding a kernel means adding a module, never
//! editing this trait to accommodate it.

pub mod claude_code;
pub mod codex;
pub mod sandbox;

use tokio::sync::mpsc;

/// Size of the runtime's terminal, in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// What a caller asks for when provisioning a runtime.
#[derive(Debug, Clone)]
pub struct RuntimeSpec {
    /// Identifier of the session the runtime serves, for correlation only.
    pub session_id: String,
    /// Geometry to allocate the terminal at.
    pub size: TermSize,
}

/// An opaque reference to a provisioned runtime.
///
/// The string is meaningful only to the adapter that minted it. Callers store
/// and return it; they must never parse it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeHandle(pub String);

/// Something sent toward the runtime's terminal.
#[derive(Debug, Clone)]
pub enum TerminalInput {
    /// Raw bytes written to the terminal, as if typed.
    Bytes(Vec<u8>),
    /// The terminal was resized.
    Resize(TermSize),
    /// An out-of-band signal, named in the conventional short form.
    Signal(String),
}

/// A bidirectional attachment to a runtime's terminal.
#[derive(Debug)]
pub struct TerminalChannel {
    /// Output produced by the runtime.
    pub output: mpsc::Receiver<Vec<u8>>,
    /// Input destined for the runtime.
    pub input: mpsc::Sender<TerminalInput>,
}

/// Errors an adapter may surface.
#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// The runtime could not be provisioned or reached.
    #[error("runtime unavailable: {0}")]
    Unavailable(String),
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

    /// Bring up a runtime for a session.
    async fn provision(&self, spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError>;

    /// Open a bidirectional attachment to the runtime's terminal.
    async fn attach_terminal(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError>;

    /// Inform the runtime that its terminal was resized.
    async fn resize(&self, handle: &RuntimeHandle, size: TermSize) -> Result<(), AdapterError>;

    /// Tear the runtime down.
    async fn terminate(&self, handle: &RuntimeHandle) -> Result<(), AdapterError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_compare_by_value() {
        assert_eq!(RuntimeHandle("a".into()), RuntimeHandle("a".into()));
        assert_ne!(RuntimeHandle("a".into()), RuntimeHandle("b".into()));
    }

    #[test]
    fn adapter_errors_carry_their_context() {
        let e = AdapterError::Unavailable("connection refused".into());
        assert!(e.to_string().contains("connection refused"));
    }
}
