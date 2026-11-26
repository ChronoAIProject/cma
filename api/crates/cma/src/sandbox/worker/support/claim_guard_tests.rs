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

#[test]
fn unschedulable_runtime_publishes_a_safe_retryable_reason() {
    let failure = adapter_failure(AdapterError::NativeClaimNotReady(
        NativeClaimGuardReason::Unschedulable,
    ));
    assert!(failure.retryable);
    assert_eq!(failure.retry_kind(), WorkRetryFailure::Transient);
    assert!(!failure.native_claim_pending);
    assert_eq!(
        failure.typed().detail,
        "runtime pod could not be scheduled; original identity retained"
    );
}
