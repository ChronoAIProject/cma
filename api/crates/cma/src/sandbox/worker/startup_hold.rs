use super::support::WorkerFailure;
use layer3_adapter::RuntimeStartupFailureClass;
use std::time::Duration;

fn detail(class: RuntimeStartupFailureClass) -> &'static str {
    match class {
        RuntimeStartupFailureClass::ExitedBeforeServing => {
            "runtime startup is held after repeated startup failures (exited_before_serving)"
        }
        RuntimeStartupFailureClass::OutOfMemory => {
            "runtime startup is held after repeated startup failures (out_of_memory)"
        }
        RuntimeStartupFailureClass::Terminated => {
            "runtime startup is held after repeated startup failures (terminated)"
        }
    }
}

pub(in crate::sandbox::worker) fn delay(base: Duration, attempts: u32) -> Duration {
    Duration::from_secs(30)
        .saturating_mul(1_u32 << attempts.min(4))
        .min(Duration::from_secs(300))
        .max(base)
}

impl WorkerFailure {
    pub(in crate::sandbox::worker) fn is_startup_hold(&self) -> bool {
        self.startup_hold
    }

    pub(super) fn startup_hold(class: RuntimeStartupFailureClass) -> Self {
        let mut failure = Self::retryable(detail(class));
        failure.startup_hold = true;
        failure
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::worker::support::adapter_failure;
    use layer2_core::sandbox::work_retry::WorkRetryFailure;
    use layer3_adapter::{AdapterError, RuntimeStartupFailure};

    #[test]
    fn startup_hold_mapping_is_retryable_and_has_only_fixed_diagnostics() {
        for class in [
            RuntimeStartupFailureClass::ExitedBeforeServing,
            RuntimeStartupFailureClass::OutOfMemory,
            RuntimeStartupFailureClass::Terminated,
        ] {
            let failure = adapter_failure(AdapterError::RuntimeStartupHold(
                RuntimeStartupFailure::new(class, u32::MAX),
            ));
            assert!(failure.is_retryable());
            assert_eq!(failure.retry_kind(), WorkRetryFailure::Transient);
            assert!(failure.is_startup_hold());
            let unavailable = adapter_failure(AdapterError::Unavailable("temporary".into()));
            assert!(!unavailable.is_startup_hold());
            assert_eq!(failure.typed().class, "retryable_transient");
            assert_eq!(failure.typed().class, unavailable.typed().class);
            assert_eq!(failure.detail(), detail(class));
        }
    }

    #[test]
    fn durable_startup_hold_attempts_keep_the_floor_and_cap() {
        assert_eq!(
            (0..7)
                .map(|n| delay(Duration::from_secs(1), n).as_secs())
                .collect::<Vec<_>>(),
            [30, 60, 120, 240, 300, 300, 300]
        );
        assert_eq!(
            delay(Duration::from_secs(600), u32::MAX),
            Duration::from_secs(600)
        );
    }
}
