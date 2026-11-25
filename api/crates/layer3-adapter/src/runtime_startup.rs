//! Bounded, content-free evidence of repeated runtime startup failure.

use crate::RuntimeHandle;
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeStartupFailureClass {
    ExitedBeforeServing,
    OutOfMemory,
    Terminated,
}

impl RuntimeStartupFailureClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExitedBeforeServing => "exited_before_serving",
            Self::OutOfMemory => "out_of_memory",
            Self::Terminated => "terminated",
        }
    }
}

impl std::fmt::Display for RuntimeStartupFailureClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeStartupFailure {
    pub class: RuntimeStartupFailureClass,
    pub consecutive: u32,
    probe: Option<RuntimeStartupProbe>,
}

/// Advisory scheduling evidence from the process that owns the startup breaker.
/// The deadline uses that process's monotonic clock and is never persisted.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeStartupProbe {
    pub runtime: RuntimeHandle,
    pub sandbox_id: String,
    pub generation: u64,
    pub next_probe_at: Instant,
}

impl std::fmt::Debug for RuntimeStartupProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeStartupProbe")
            .finish_non_exhaustive()
    }
}

impl RuntimeStartupFailure {
    pub fn new(class: RuntimeStartupFailureClass, consecutive: u32) -> Self {
        Self {
            class,
            consecutive,
            probe: None,
        }
    }

    pub fn with_probe(mut self, probe: RuntimeStartupProbe) -> Self {
        self.probe = Some(probe);
        self
    }

    pub fn probe(&self) -> Option<&RuntimeStartupProbe> {
        self.probe.as_ref()
    }
}

impl std::fmt::Display for RuntimeStartupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}; consecutive={}", self.class, self.consecutive)
    }
}

/// Opaque identities supplied by the runtime provider, with no format interpretation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartupIncarnation {
    pub boot: String,
    pub process: String,
    pub restart: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct StartupTermination {
    pub process: String,
    pub failed: bool,
    pub class: RuntimeStartupFailureClass,
    pub finished_at: std::time::SystemTime,
}

/// Previous termination from the currently proved runtime container's history.
#[derive(Clone, Debug)]
pub(crate) struct StartupHistory {
    pub class: RuntimeStartupFailureClass,
    pub duration: std::time::Duration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AdapterError;

    #[test]
    fn startup_hold_is_content_free_and_preserves_loss_semantics_and_context() {
        for (class, label) in [
            (
                RuntimeStartupFailureClass::ExitedBeforeServing,
                "exited_before_serving",
            ),
            (RuntimeStartupFailureClass::OutOfMemory, "out_of_memory"),
            (RuntimeStartupFailureClass::Terminated, "terminated"),
        ] {
            let failure = RuntimeStartupFailure::new(class, 3);
            assert_eq!(class.as_str(), label);
            let error = AdapterError::RuntimeStartupHold(failure);
            assert!(error.is_runtime_loss());
            assert_eq!(error.clone().context("unretained runtime text"), error);
            assert_eq!(
                error.to_string(),
                format!("runtime startup hold: {label}; consecutive=3")
            );
        }
    }
}
