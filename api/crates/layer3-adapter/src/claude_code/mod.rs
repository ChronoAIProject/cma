//! Adapter driving the Claude Code kernel.
//!
//! Not implemented in this version. The module exists so that adding the kernel
//! is a matter of filling this in, never of editing the trait it implements.

use crate::{AdapterError, KernelAdapter, RuntimeHandle, RuntimeSpec, TermSize, TerminalChannel};

/// Placeholder adapter. Every operation reports the kernel as unavailable.
pub struct ClaudeCodeAdapter;

#[async_trait::async_trait]
impl KernelAdapter for ClaudeCodeAdapter {
    fn kernel_name(&self) -> &'static str {
        "claude_code"
    }

    async fn provision(&self, _spec: &RuntimeSpec) -> Result<RuntimeHandle, AdapterError> {
        Err(AdapterError::Unavailable("kernel not implemented".into()))
    }

    async fn attach_terminal(
        &self,
        _handle: &RuntimeHandle,
    ) -> Result<TerminalChannel, AdapterError> {
        Err(AdapterError::Unavailable("kernel not implemented".into()))
    }

    async fn resize(&self, _handle: &RuntimeHandle, _size: TermSize) -> Result<(), AdapterError> {
        Err(AdapterError::Unavailable("kernel not implemented".into()))
    }

    async fn terminate(&self, _handle: &RuntimeHandle) -> Result<(), AdapterError> {
        Err(AdapterError::Unavailable("kernel not implemented".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_operation_reports_unavailable() {
        let a = ClaudeCodeAdapter;
        assert_eq!(a.kernel_name(), "claude_code");

        let spec = RuntimeSpec {
            session_id: "sesn_1".into(),
            size: TermSize { cols: 80, rows: 24 },
        };
        assert!(a.provision(&spec).await.is_err());

        let handle = RuntimeHandle("sesn_1".into());
        assert!(a.attach_terminal(&handle).await.is_err());
        assert!(a
            .resize(&handle, TermSize { cols: 80, rows: 24 })
            .await
            .is_err());
        assert!(a.terminate(&handle).await.is_err());
    }
}
