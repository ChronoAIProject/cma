use super::*;
use crate::sandbox::{SandboxInterruptionCause, SandboxLifecycleState};

#[tokio::test]
async fn interruption_cause_is_committed_with_recovery_and_stale_retry_cannot_replace_it() {
    let (store, original) = fixture(vec![], true);
    let mut request = loss_request(&original);
    request.interruption_cause = SandboxInterruptionCause::OutOfMemory;
    store.recover_runtime_loss(request.clone()).await.unwrap();
    let recovered = store.get_sandbox(&original.id).await.unwrap().unwrap();
    let interruption = recovered.lifecycle.interruption.as_ref().unwrap();
    assert_eq!(interruption.cause, SandboxInterruptionCause::OutOfMemory);
    assert_eq!(interruption.detected_at, request.observed_at);
    assert_eq!(
        interruption.memory_limit_mib,
        original.profile.content.memory_mib
    );
    assert_eq!(
        recovered
            .lifecycle
            .operation
            .as_ref()
            .unwrap()
            .authority()
            .requested_at,
        interruption.detected_at
    );
    request.interruption_cause = SandboxInterruptionCause::RuntimeUnavailable;
    assert_eq!(
        store.recover_runtime_loss(request).await.unwrap(),
        SandboxRuntimeLossOutcome::Superseded
    );
    assert_eq!(
        store
            .get_sandbox(&original.id)
            .await
            .unwrap()
            .unwrap()
            .lifecycle
            .interruption,
        recovered.lifecycle.interruption
    );
}

#[test]
fn old_lifecycle_records_remain_readable_and_optional_interruption_round_trips() {
    let state = SandboxLifecycleState::default();
    let old = serde_json::to_value(&state).unwrap();
    assert!(old.get("interruption").is_none());
    assert_eq!(
        serde_json::from_value::<SandboxLifecycleState>(old).unwrap(),
        state
    );
}
