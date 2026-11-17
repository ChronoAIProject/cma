//! Bounded, content-free evidence of repeated runtime startup failure.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeStartupFailure {
    pub class: RuntimeStartupFailureClass,
    pub consecutive: u32,
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
            let failure = RuntimeStartupFailure {
                class,
                consecutive: 3,
            };
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
