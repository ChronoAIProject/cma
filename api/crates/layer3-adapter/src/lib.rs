//! Temporary Layer 3 adapter boundary for the CMA resource-model rebuild.
//!
//! Issue #806 rewrites the hosted runtime, provider adapter, sandbox, and
//! supervisor contracts against the aggregates introduced by #802. Keeping a
//! small provider-neutral boundary here lets every commit in the teardown
//! series build without guessing those contracts early.

/// Static abilities advertised by a runtime implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KernelCapabilities {
    pub conversation: bool,
    pub snapshot: bool,
    pub steer: bool,
    pub interrupt: bool,
    pub usage: bool,
    pub resources: bool,
}

/// A provider-neutral adapter failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    #[error("runtime unavailable: {0}")]
    Unavailable(String),
    #[error("runtime rejected request: {0}")]
    Rejected(String),
    #[error("no capacity for the requested runtime: {0}")]
    AtCapacity(String),
    #[error("runtime capability is not supported: {0}")]
    Unsupported(String),
}

impl AdapterError {
    /// Whether the failure is evidence that the runtime no longer answers.
    pub fn is_runtime_loss(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }

    /// Add diagnostic context without changing the failure category.
    pub fn context(self, context: &str) -> Self {
        match self {
            Self::Unavailable(message) => Self::Unavailable(format!("{context}: {message}")),
            Self::Rejected(message) => Self::Rejected(format!("{context}: {message}")),
            Self::AtCapacity(message) => Self::AtCapacity(format!("{context}: {message}")),
            Self::Unsupported(message) => Self::Unsupported(format!("{context}: {message}")),
        }
    }
}

/// Minimal registry-facing contract retained until #806 defines provisioning.
pub trait KernelAdapter: Send + Sync {
    fn kernel_name(&self) -> &'static str;
    fn capabilities(&self) -> KernelCapabilities;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_context_preserves_runtime_loss_evidence() {
        let error = AdapterError::Unavailable("closed".into()).context("attach");
        assert!(error.is_runtime_loss());
        assert_eq!(error.to_string(), "runtime unavailable: attach: closed");
    }
}
