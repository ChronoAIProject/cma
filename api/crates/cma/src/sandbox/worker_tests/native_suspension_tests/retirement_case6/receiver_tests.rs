use super::*;

pub(super) async fn single_manager_admits_retirement_without_child_receiver_capture() {
    use crate::store::native_retirement_action::{RetirementPhase, RetirementRecords};

    let (f, active, _, mut now) = active_siblings().await;
    let receiver = Receiver::new(&f, &active).await;
    admit(&f, &active).await;
    let clock = Arc::new(Mutex::new(now));
    let worker = parent_worker_with_unreachable_probe(&f, clock.clone());
    f.backend.hold_finality.store(true, Ordering::SeqCst);
    let mut action = None;
    for _ in 0..10 {
        now += Duration::from_secs(6);
        *clock.lock().unwrap() = now + Duration::from_secs(1);
        worker.run_one(&active.id, now).await.unwrap();
        action = f
            .backend
            .retirement_records
            .action(&active.id)
            .await
            .unwrap();
        if action.is_some() {
            break;
        }
    }
    let action = action.expect("the parent admits a fresh receiver probe");
    assert_eq!(action.phase, RetirementPhase::Dispatching);
    assert!(action.observation.receiver.pod.is_uid_only());
    assert!(receiver.calls.load(Ordering::SeqCst) >= 2);
    assert_eq!(
        f.harness.stored(&active.id).await.status,
        SandboxStatus::Retiring
    );
    f.backend.hold_finality.store(false, Ordering::SeqCst);
    for _ in 0..10 {
        now += Duration::from_secs(6);
        *clock.lock().unwrap() = now + Duration::from_secs(1);
        worker.run_one(&active.id, now).await.unwrap();
        if f.harness.stored(&active.id).await.status == SandboxStatus::Retired {
            break;
        }
    }
    assert_eq!(
        f.harness.stored(&active.id).await.status,
        SandboxStatus::Retired
    );
    assert!(f
        .backend
        .retirement_records
        .action(&active.id)
        .await
        .unwrap()
        .is_none());
}
