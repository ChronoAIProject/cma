use super::*;
use crate::sandbox::worker::SandboxWorker;
use crate::store::native_retirement_action::{RetirementAction, RetirementRecords};

pub(super) async fn resume(
    f: &Fixture,
    active: &Sandbox,
    mut now: SystemTime,
    clock: &Mutex<SystemTime>,
    successor: &SandboxWorker,
    action: RetirementAction,
) -> SystemTime {
    use crate::sandbox::credential::CredentialError;
    let before = f.harness.stored(&active.id).await;
    *f.harness.credentials.settlement_failure.lock().unwrap() = Some(CredentialError::Uncertain);
    now = now.max(stamp_time(&before.work.ready_at));
    *clock.lock().unwrap() = now + Duration::from_secs(1);
    assert!(successor.run_one(&active.id, now).await.unwrap());
    let blocked = f.harness.stored(&active.id).await;
    assert_eq!(blocked.status, SandboxStatus::Retiring);
    assert_eq!(blocked.work.step, SandboxWorkStep::StopAgents);
    assert!(blocked.work.lease.is_none());
    assert_eq!(blocked.credential, before.credential);
    assert_eq!(blocked.runtime, before.runtime);
    assert_eq!(blocked.lifecycle, before.lifecycle);
    assert_eq!(
        f.backend
            .retirement_records
            .action(&active.id)
            .await
            .unwrap(),
        Some(action.clone())
    );
    assert!(f.backend.calls.snapshot().contains(&"suspend".into()));
    assert!(!f
        .backend
        .calls
        .snapshot()
        .contains(&"delete-retained".into()));
    assert!(!f.backend.home_destroyed.load(Ordering::SeqCst));

    *f.harness.credentials.settlement_failure.lock().unwrap() = None;
    f.backend.hold_finality.store(true, Ordering::SeqCst);
    now = stamp_time(&blocked.work.ready_at);
    *clock.lock().unwrap() = now + Duration::from_secs(1);
    assert!(successor.run_one(&active.id, now).await.unwrap());
    let awaiting_finality = f.harness.stored(&active.id).await;
    assert_eq!(awaiting_finality.runtime, before.runtime);
    assert_eq!(awaiting_finality.lifecycle.home, before.lifecycle.home);
    assert_eq!(
        f.backend
            .retirement_records
            .action(&active.id)
            .await
            .unwrap(),
        Some(action)
    );
    assert!(f
        .backend
        .calls
        .snapshot()
        .contains(&"retire-suspended".into()));
    f.backend.hold_finality.store(false, Ordering::SeqCst);
    stamp_time(&awaiting_finality.work.ready_at)
}
