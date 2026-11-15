use super::*;
use layer3_adapter::agent_sandbox::NativeClaimGuardReason;

#[test]
fn readiness_guard_remains_retryable_without_replacement_authority() {
    for reason in [
        NativeClaimGuardReason::TimedOut,
        NativeClaimGuardReason::CrashLoop,
        NativeClaimGuardReason::Restarting,
    ] {
        let failure = adapter_failure(AdapterError::NativeClaimNotReady(reason));
        assert!(failure.retryable);
        assert_eq!(failure.retry_kind(), WorkRetryFailure::Transient);
    }
}
