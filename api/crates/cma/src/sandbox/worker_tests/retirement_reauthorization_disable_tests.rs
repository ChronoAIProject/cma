use super::*;
use layer2_core::sandbox::work_retry::WORK_RETRY_PROBE_INTERVAL;
use layer2_core::sandbox::{SandboxRetirement, StoredSandboxRetirement};

#[tokio::test]
async fn disabling_reauthorization_restores_stopped_retirement() {
    let h = Harness::new(Vec::new(), 0, Duration::from_secs(30)).await;
    let parked = park_for_owner_reauthorization(&h, true).await;
    assert!(parked.work.retirement_credential_recovery.is_some());
    let prior_actions = h.actions.snapshot();
    h.credentials
        .require_retirement_authority
        .store(false, Ordering::SeqCst);
    let due = stamp_time(&parked.work.ready_at);
    h.run(&parked.id, due).await;
    let stopped = h.stored(&parked.id).await;
    assert!(stopped.retirement_retry_exhausted());
    assert!(stopped.work.retirement_credential_recovery.is_none());
    assert_eq!(
        stopped.credential.revoke_debt,
        parked.credential.revoke_debt
    );
    assert_eq!(stopped.work.step, parked.work.step);
    assert!(!stopped.retirement_released_owner_slot());
    let later = due + WORK_RETRY_PROBE_INTERVAL;
    assert!(!h.worker.run_one(&parked.id, later).await.unwrap());
    assert_eq!(h.stored(&parked.id).await.revision, stopped.revision);
    assert_eq!(h.actions.snapshot(), prior_actions);

    let resumed = StoredSandboxRetirement::new(h.store.clone())
        .retire(&parked.id, rfc3339_at(later), Some(stopped.revision))
        .await
        .unwrap();
    assert!(resumed.changed);
    assert_eq!(
        resumed.sandbox.credential.revoke_debt,
        parked.credential.revoke_debt
    );
}
