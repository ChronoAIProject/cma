use super::*;
use layer2_core::sandbox::store::AgentWorkerStore;

mod credential_hold;

async fn failed_close() -> (Fixture, Sandbox, Vec<Agent>, SystemTime, Owners, Receiver) {
    let (f, active, children, mut now) = active_siblings().await;
    let owners = Owners::new().await;
    let receiver = Receiver::new(&f, &active).await;
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

pub(super) async fn recovery_between_admission_and_dispatch_returns_to_graceful_stop() {
    use crate::store::native_retirement_action::RetirementRecords;
    let (f, active, children, mut now, owners, receiver) = failed_close().await;
    let before = receiver.calls.load(Ordering::SeqCst);
    receiver.recover_after.store(before + 1, Ordering::SeqCst);
    now += Duration::from_secs(6);
    let clock = Arc::new(Mutex::new(now + Duration::from_secs(1)));
    let worker = finalizing_parent_worker(&f, clock);
    worker.run_one(&active.id, now).await.unwrap();
    assert_eq!(receiver.calls.load(Ordering::SeqCst), before + 2);
    assert_eq!(
        f.backend
            .retirement_records
            .action(&active.id)
            .await
            .unwrap(),
        None
    );
    assert!(!f
        .backend
        .calls
        .snapshot()
        .contains(&"retire-alternative".into()));
    blocked(&f, &active, &children).await;
    receiver.server.abort();
    owners.cleanup().await;
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
                f.backend.retirement_records.clone(),
                false,
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
        f.backend.retirement_records.clone(),
        true,
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

pub(super) async fn provider_retirement_refuses_a_child_writer_that_wins_admission() {
    use crate::sandbox::native_retirement_finalizer::NativeRetirementFinalizer;
    use crate::store::native_retirement_action::RetirementRecords;
    use crate::store::sandbox_maintenance::mongo_integration_tests::database;

    for attach in [false, true] {
        let (f, active, children, _, owners, receiver) = failed_close().await;
        let (client, database, child_store) = database().await;
        let parent_store = crate::store::MongoSandboxStore::open(&client, &database)
            .await
            .unwrap();
        let mut parent = f.harness.stored(&active.id).await;
        parent.work.lease = None;
        parent.revision = 1;
        parent.updated_at = rfc3339_at(SystemTime::now());
        parent.work.ready_at = parent.updated_at.clone();
        child_store.create_sandbox(parent.clone()).await.unwrap();
        for mut child in children {
            child.revision = 1;
            child.conversation.last_seq = 0;
            child_store.create_agent(child).await.unwrap();
        }
        let observation = f
            .backend
            .retirement_records
            .observation(&parent.id)
            .await
            .unwrap()
            .unwrap()
            .0;
        child_store.note(&parent, &observation).await.unwrap();
        let now = SystemTime::now();
        let claimed = parent_store
            .claim_sandbox_work(
                &parent.id,
                "acting-replica",
                rfc3339_at(now),
                rfc3339_at(now + Duration::from_secs(180)),
            )
            .await
            .unwrap()
            .unwrap();
        assert!(claimed.work.attempts >= 3);
        assert!(parent_store.action(&parent.id).await.unwrap().is_none());

        let pause = Arc::new(super::super::backend::RetirementProbePause::default());
        *f.backend.retirement_probe_pause.lock().unwrap() = Some(pause.clone());
        let parent_store = Arc::new(parent_store);
        let finalizer = NativeRetirementFinalizer::new(
            parent_store.clone(),
            f.harness.adapter.clone(),
            "test".into(),
            parent_store.clone(),
            true,
        );
        let pending =
            tokio::spawn(async move { finalizer.finalize(&claimed, "acting-replica").await });
        tokio::time::timeout(Duration::from_secs(8), pause.entered.notified())
            .await
            .expect("acting replica reached the fresh receiver exchange");
        let original_id = layer2_core::AgentId(observation.receiver.owner.agent_id);
        let mut child = child_store.get_agent(&original_id).await.unwrap().unwrap();
        if attach {
            child.id = layer2_core::AgentId("agt_late_retirement_child".into());
            child.worktree_id = layer2_core::WorktreeId("wkt_late_retirement_child".into());
            child.creation_key = "late-retirement-child".into();
            child_store.create_agent(child).await.unwrap();
        } else {
            let revision = child.revision;
            child.revision += 1;
            child
                .work
                .cleanup_debt
                .push(layer2_core::sandbox::AgentCleanupDebt {
                    step: AgentWorkStep::DispatchFirstMessage,
                    operation_id: "new-external-obligation".into(),
                    request_sha256: "c".repeat(64),
                });
            child_store.save_agent(revision, child).await.unwrap();
        }

        pause.release.notify_one();
        assert!(matches!(
            pending.await.unwrap(),
            Err(layer2_core::sandbox::store::SandboxStoreError::Conflict)
        ));
        assert_eq!(parent_store.action(&parent.id).await.unwrap(), None);
        assert!(!f
            .backend
            .calls
            .snapshot()
            .contains(&"retire-alternative".into()));
        assert!(f
            .harness
            .adapter
            .runtime_inventory()
            .await
            .unwrap()
            .iter()
            .any(|runtime| runtime.handle.as_str() == parent.runtime.as_ref().unwrap().runtime_id));
        client.database(&database).drop().await.unwrap();
        receiver.server.abort();
        owners.cleanup().await;
    }
}

pub(super) async fn disabled_admission_preserves_legacy_absent_runtime_retirement() {
    use crate::store::native_retirement_action::RetirementRecords;
    let (f, active, _, mut now) = active_siblings().await;
    admit(&f, &active).await;
    f.backend.lose_object().await;
    now += Duration::from_secs(6);
    let clock = Arc::new(Mutex::new(now + Duration::from_secs(1)));
    let worker = parent_worker_with_retirement_admission(&f, clock, false);
    worker.run_one(&active.id, now).await.unwrap();
    assert!(f
        .backend
        .calls
        .snapshot()
        .contains(&"delete-retained".into()));
    assert!(f
        .backend
        .retirement_records
        .action(&active.id)
        .await
        .unwrap()
        .is_none());
}

pub(super) async fn recovered_dispatch_resumes_graceful_stop_with_admission_disabled(
    credential_hold: bool,
) {
    use crate::store::native_retirement_action::{RetirementPhase, RetirementRecords};
    let (f, active, children, mut now, owners, receiver) = failed_close().await;
    let clock = Arc::new(Mutex::new(now + Duration::from_secs(7)));
    let original = finalizing_parent_worker(&f, clock.clone());
    f.backend
        .retirement_records
        .lose_dispatch_acknowledgement
        .store(true, Ordering::SeqCst);
    now += Duration::from_secs(6);
    assert!(matches!(
        original.run_one(&active.id, now).await,
        Err(layer2_core::sandbox::store::SandboxStoreError::Unavailable(
            _
        ))
    ));
    let action = f
        .backend
        .retirement_records
        .action(&active.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(action.phase, RetirementPhase::Dispatching);
    assert!(!f
        .backend
        .calls
        .snapshot()
        .contains(&"retire-alternative".into()));

    // The next replica cannot infer that the older call was unsent.
    // It closes the original Agents while the same action stays durable.
    receiver.recover_after.store(0, Ordering::SeqCst);
    *f.backend.close_transport.lock().unwrap() = None;
    *f.backend.gc_credentials.lock().unwrap() = Some(f.harness.credentials.clone());
    let successor = parent_worker_with_retirement_admission(&f, clock.clone(), false);
    let child = child_worker(&f, None, "recovered-child");
    now += Duration::from_secs(1200);
    *clock.lock().unwrap() = now + Duration::from_secs(1);
    successor.run_one(&active.id, now).await.unwrap();
    assert!(!f
        .backend
        .calls
        .snapshot()
        .contains(&"retire-alternative".into()));
    assert_eq!(
        f.backend
            .retirement_records
            .action(&active.id)
            .await
            .unwrap(),
        Some(action.clone())
    );

    for _ in 0..6 {
        now += Duration::from_secs(6);
        for agent in &children {
            child.run_one(&agent.id, now).await.unwrap();
        }
    }
    for agent in &children {
        let stopped = f.harness.store.get_agent(&agent.id).await.unwrap().unwrap();
        assert_eq!(stopped.status, AgentStatus::Stopped);
        assert!(stopped
            .work
            .receipts
            .iter()
            .any(|receipt| receipt.step == AgentWorkStep::CloseProcess));
    }
    assert!(!f
        .backend
        .calls
        .snapshot()
        .contains(&"retire-alternative".into()));
    if credential_hold {
        now = credential_hold::resume(&f, &active, now, &clock, &successor, action).await;
    }
    for _ in 0..10 {
        now += Duration::from_secs(6);
        *clock.lock().unwrap() = now + Duration::from_secs(1);
        successor.run_one(&active.id, now).await.unwrap();
        if f.harness.stored(&active.id).await.status == SandboxStatus::Retired {
            break;
        }
    }
    let retired = f.harness.stored(&active.id).await;
    assert_eq!(retired.status, SandboxStatus::Retired);
    assert!(retired.runtime.is_none());
    assert!(f
        .backend
        .retirement_records
        .action(&active.id)
        .await
        .unwrap()
        .is_none());
    let calls = f.backend.calls.snapshot();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "retire-alternative")
            .count(),
        1
    );
    receiver.server.abort();
    owners.cleanup().await;
}
