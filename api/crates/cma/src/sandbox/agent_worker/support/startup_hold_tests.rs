use super::*;
use layer2_core::sandbox::store::{AGENT_RUNTIME_STARTUP_HOLD, AGENT_RUNTIME_STARTUP_HOLD_DETAIL};
use layer3_adapter::{RuntimeStartupFailure, RuntimeStartupFailureClass};

#[test]
fn startup_hold_is_transient_for_starting_and_cleanup_work_without_runtime_text() {
    for map in [adapter_failure, cleanup_failure] {
        for class in [
            RuntimeStartupFailureClass::ExitedBeforeServing,
            RuntimeStartupFailureClass::OutOfMemory,
            RuntimeStartupFailureClass::Terminated,
        ] {
            let failure = map(AdapterError::RuntimeStartupHold(RuntimeStartupFailure {
                class,
                consecutive: u32::MAX,
            })
            .context("unretained context"));
            assert!(failure.retryable);
            assert_eq!(failure.retry_kind, WorkRetryFailure::Transient);
            assert_eq!(failure.typed().class, AGENT_RUNTIME_STARTUP_HOLD);
            assert_eq!(failure.typed().detail, AGENT_RUNTIME_STARTUP_HOLD_DETAIL);
            assert!(failure.is_runtime_hold());
            assert!(!failure.is_ledger_hold());
        }
    }
}
