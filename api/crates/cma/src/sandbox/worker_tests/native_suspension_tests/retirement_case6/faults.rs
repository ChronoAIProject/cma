use super::*;
use layer2_core::sandbox::store::AgentWorkerStore;

async fn failed_close() -> (Fixture, Sandbox, Vec<Agent>, SystemTime, Owners, Receiver) {
    let (f, active, children, mut now) = active_siblings().await;
    let owners = Owners::new().await;
    let receiver = Receiver::new().await;
    *f.backend.close_transport.lock().unwrap() = Some(receiver.transport.clone());
    admit(&f, &active).await;
    let parent = parent_worker(&f);
    for _ in 0..3 {
        now += Duration::from_secs(6);
        parent.run_one(&active.id, now).await.unwrap();
        for (index, owner) in owners.coordinators.iter().enumerate() {
            let worker = child_worker(&f, Some(owner.clone()), &format!("fault-{index}"));
            for child in &children {
                let _ = worker.run_one(&child.id, now).await;
            }
        }
    }
    blocked(&f, &active, &children).await;
    assert!(receiver.calls.load(Ordering::SeqCst) > 0);
    (f, active, children, now, owners, receiver)
}

pub(super) async fn retirement_case6_retained_claim_survives_proof_failure_then_settles_without_expiry(
) {
    for lost_delete in [false, true] {
        let (f, active, children, mut now, owners, receiver) = failed_close().await;
        now += Duration::from_secs(6);
        let abandoned_until = now + Duration::from_secs(1080);
        for child in &children {
            assert!(f
                .harness
                .store
                .claim_agent_work(
                    &child.id,
                    "abandoned-work",
                    rfc3339_at(now),
                    rfc3339_at(abandoned_until)
                )
                .await
                .unwrap()
                .is_some());
        }
        f.backend
            .hold_finality
            .store(!lost_delete, Ordering::SeqCst);
        f.backend.lose_delete.store(lost_delete, Ordering::SeqCst);
        let clock = Arc::new(Mutex::new(now + Duration::from_secs(1)));
        let worker = finalizing_parent_worker(&f, clock.clone());
        worker.run_one(&active.id, now).await.unwrap();
        let pending = f.harness.stored(&active.id).await;
        assert_eq!(pending.status, SandboxStatus::Retiring);
        assert_eq!(pending.runtime, active.runtime);
        assert_eq!(pending.credential, active.credential);
        assert_eq!(pending.lifecycle.home, active.lifecycle.home);
        assert!(f
            .harness
            .adapter
            .runtime_inventory()
            .await
            .unwrap()
            .is_empty());
        for child in &children {
            let stored = f.harness.store.get_agent(&child.id).await.unwrap().unwrap();
            assert_eq!(stored.status, AgentStatus::Ready);
            assert_eq!(stored.work.lease.as_ref().unwrap().holder, "abandoned-work");
        }
        // A fresh process has no captured receiver retry handle. Physical absence
        // remains independently provable from the unchanged original binding.
        let restarted_service =
            crate::sandbox::native_retirement_finalizer::NativeRetirementFinalizer::new(
                f.harness.store.clone(),
                f.harness.adapter.clone(),
                "test".into(),
                Default::default(),
            )
            .with_clock({
                let clock = clock.clone();
                Arc::new(move || *clock.lock().unwrap())
            });
        f.backend.hold_finality.store(false, Ordering::SeqCst);
        now += Duration::from_secs(6);
        *clock.lock().unwrap() = now + Duration::from_secs(1);
        let claimed = f
            .harness
            .store
            .claim_sandbox_work(
                &active.id,
                "restart",
                rfc3339_at(now),
                rfc3339_at(now + Duration::from_secs(60)),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            restarted_service
                .finalize(&claimed, "restart")
                .await
                .unwrap(),
            crate::sandbox::native_retirement_finalizer::RetirementOutcome::Settled
        ));
        // Never advance the stale claimed parent after aggregate commit.
        assert!(matches!(
            restarted_service
                .finalize(&claimed, "restart")
                .await
                .unwrap(),
            crate::sandbox::native_retirement_finalizer::RetirementOutcome::Superseded
        ));
        for _ in 0..10 {
            now += Duration::from_secs(6);
            *clock.lock().unwrap() = now + Duration::from_secs(1);
            worker.run_one(&active.id, now).await.unwrap();
            if f.harness.stored(&active.id).await.status == SandboxStatus::Retired {
                break;
            }
        }
        assert!(now < abandoned_until);
        assert_eq!(
            f.harness.stored(&active.id).await.status,
            SandboxStatus::Retired
        );
        for child in &children {
            let after = f.harness.store.get_agent(&child.id).await.unwrap().unwrap();
            assert_eq!(after.status, AgentStatus::Stopped);
            assert!(after.work.lease.is_none());
        }
        receiver.server.abort();
        owners.cleanup().await;
    }
}

pub(super) async fn retirement_case6_refuses_external_debt_and_active_parent_control() {
    let (f, active, children, mut now, owners, receiver) = failed_close().await;
    let mut child = f
        .harness
        .store
        .get_agent(&children[0].id)
        .await
        .unwrap()
        .unwrap();
    let revision = child.revision;
    child
        .work
        .cleanup_debt
        .push(layer2_core::sandbox::AgentCleanupDebt {
            step: AgentWorkStep::DispatchFirstMessage,
            operation_id: "external-pending".into(),
            request_sha256: "a".repeat(64),
        });
    child.revision += 1;
    f.harness
        .store
        .save_agent(revision, child.clone())
        .await
        .unwrap();
    let clock = Arc::new(Mutex::new(now));
    let worker = finalizing_parent_worker(&f, clock.clone());
    for _ in 0..3 {
        now += Duration::from_secs(6);
        *clock.lock().unwrap() = now + Duration::from_secs(1);
        worker.run_one(&active.id, now).await.unwrap();
    }
    blocked(&f, &active, &children).await;
    assert_eq!(
        f.harness.store.get_agent(&child.id).await.unwrap(),
        Some(child)
    );
    let service = crate::sandbox::native_retirement_finalizer::NativeRetirementFinalizer::new(
        f.harness.store.clone(),
        f.harness.adapter.clone(),
        "test".into(),
        f.backend.retirement_failures.clone(),
    );
    let mut ordinary = active.clone();
    ordinary.work.attempts = 10;
    assert!(matches!(
        service.finalize(&ordinary, "unrelated").await.unwrap(),
        crate::sandbox::native_retirement_finalizer::RetirementOutcome::Superseded
    ));
    assert!(!f
        .backend
        .calls
        .snapshot()
        .contains(&"retire-alternative".into()));
    receiver.server.abort();
    owners.cleanup().await;
}
