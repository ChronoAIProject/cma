use super::*;
use crate::sandbox::native_retirement_delivery::CaptureDelivery;
use crate::store::native_retirement_action::{
    RetirementAction, RetirementObservation, RetirementRecords,
};
use layer2_core::sandbox::store::{AgentControlStore, AgentControlTarget, AgentStopRequest};
use layer2_core::sandbox::AgentStopReason;
use layer3_adapter::receiver_failure::RetirementReceiver;

async fn run_child_due(
    worker: &AgentWorker,
    child: &layer2_core::AgentId,
) -> Result<bool, SandboxStoreError> {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let result = worker.run_one(child, SystemTime::now()).await;
            if result != Ok(false) {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("child becomes eligible for the next guarded attempt")
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn retirement_capture_at_capacity_allows_ordinary_agent_stop() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let client = Client::with_uri_str(uri).await.unwrap();
    let name = format!("retirement_capacity_{}", Uuid::new_v4().simple());
    let base = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let coordinator = OwnershipCoordinator::register(
        Arc::new(MongoAgentOwnership::open(&client, &name).await.unwrap()),
        member(),
    )
    .await
    .unwrap();
    let owned = Arc::new(
        (*base)
            .clone()
            .enable_agent_ownership("child-owner".into())
            .await
            .unwrap(),
    );
    let delivery = CaptureDelivery::new(owned.clone(), 1);
    let (release, waiting) = tokio::sync::oneshot::channel::<()>();
    assert!(delivery.tasks.spawn(async move {
        waiting.await.unwrap();
    }));

    let (retiring, mut parent, mut children, _) = active_siblings().await;
    let mut child = children.remove(0);
    let now = SystemTime::now();
    composed::seed(&retiring, &base, &mut parent, &mut child, now).await;
    let worker =
        composed::parent_worker(&retiring, base.clone(), "capacity-parent", Some(&delivery));
    composed::run_due(&worker, &parent.id).await;

    let worker = composed::child_worker(
        &retiring,
        owned.clone(),
        coordinator.clone(),
        delivery.clone(),
    );
    assert!(run_child_due(&worker, &child.id).await.unwrap());
    assert_eq!(
        owned.get_agent(&child.id).await.unwrap().unwrap().work.step,
        AgentWorkStep::CloseProcess
    );
    let (transport, exchanges, server) = receiver().await;
    *retiring.backend.inspection_transport.lock().unwrap() = Some(transport.clone());
    *retiring.backend.close_transport.lock().unwrap() = Some(transport);
    assert!(run_child_due(&worker, &child.id).await.unwrap());
    let deferred = owned.get_agent(&child.id).await.unwrap().unwrap();
    assert_eq!(deferred.status, AgentStatus::Ready);
    assert_eq!(deferred.work.step, AgentWorkStep::CloseProcess);
    assert_eq!(deferred.work.attempts, 1);
    assert!(deferred.work.lease.is_none());
    assert!(!deferred
        .work
        .receipts
        .iter()
        .any(|receipt| receipt.step == AgentWorkStep::CloseProcess));
    assert!(exchanges.lock().unwrap().is_empty());
    let row = client
        .database(&name)
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":parent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(phase(&row), "offered");

    let (healthy, mut active, mut children, _) = active_siblings().await;
    active.name = "Healthy worker sandbox".into();
    let mut control = children.pop().unwrap();
    let now = SystemTime::now();
    composed::seed_active(&healthy, &base, &mut active, &mut control, now).await;
    base.schedule_agent_stop(AgentStopRequest {
        target: AgentControlTarget::from(&control),
        reason: AgentStopReason::Owner,
        requested_at: rfc3339_at(now),
    })
    .await
    .unwrap();
    let worker = composed::child_worker(&healthy, owned.clone(), coordinator, delivery.clone());
    for _ in 0..3 {
        assert!(run_child_due(&worker, &control.id).await.unwrap());
    }
    let stopped = owned.get_agent(&control.id).await.unwrap().unwrap();
    assert_eq!(stopped.status, AgentStatus::Stopped);
    assert!(stopped
        .work
        .receipts
        .iter()
        .any(|receipt| receipt.step == AgentWorkStep::CloseProcess));
    assert!(!delivery.tasks.spawn(async {}));
    release.send(()).unwrap();
    delivery.tasks.drain(Duration::from_secs(1)).await.unwrap();
    server.abort();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn retirement_with_deleted_children_preserves_cleanup_and_action_paths() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let client = Client::with_uri_str(uri).await.unwrap();
    for (capture, custody) in [(false, false), (true, false), (false, true), (true, true)] {
        let name = format!("retirement_deleted_{}", Uuid::new_v4().simple());
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let (f, mut parent, mut children, _) = active_siblings().await;
        let now = SystemTime::now();
        let mut child = children.remove(0);
        child
            .transition(
                AgentStatus::Stopped,
                rfc3339_at(now),
                Some(AgentStopReason::SandboxRetired),
            )
            .unwrap();
        child
            .transition(AgentStatus::Deleted, rfc3339_at(now), None)
            .unwrap();
        composed::seed(&f, &store, &mut parent, &mut child, now).await;
        let delivery = CaptureDelivery::new(store.clone(), 1);
        let receiver = Receiver::new(&f, &parent).await;
        let worker = composed::parent_worker(
            &f,
            store.clone(),
            "deleted-parent",
            capture.then_some(&delivery),
        );

        if custody {
            let observation = RetirementObservation {
                owner: parent.owner.clone(),
                binding: parent.runtime.clone().unwrap(),
                receiver: RetirementReceiver {
                    runtime: RuntimeHandle::new(&parent.runtime.as_ref().unwrap().runtime_id),
                    pod: super::super::super::records::pod(),
                    owner: TransportOwner {
                        protocol: PROTOCOL_VERSION,
                        agent_id: child.id.to_string(),
                        instance_id: "historical-owner".into(),
                        epoch: 1,
                    },
                },
            };
            store.note(&parent, &observation).await.unwrap();
            let (_, fences) = store.observation(&parent.id).await.unwrap().unwrap();
            let now = SystemTime::now();
            let claim = store
                .claim_sandbox_work(
                    &parent.id,
                    "fixture-action",
                    rfc3339_at(now),
                    rfc3339_at(now + Duration::from_secs(2)),
                )
                .await
                .unwrap()
                .unwrap();
            let action =
                RetirementAction::new(&claim, observation, std::slice::from_ref(&child), fences)
                    .unwrap();
            RetirementRecords::admit(store.as_ref(), &claim, &action)
                .await
                .unwrap();
            f.backend.hold_finality.store(true, Ordering::SeqCst);
            composed::run_due(&worker, &parent.id).await;
            assert!(store.action(&parent.id).await.unwrap().is_some());
            f.backend.hold_finality.store(false, Ordering::SeqCst);
        } else {
            f.backend.deny_inventory.store(true, Ordering::SeqCst);
            composed::run_due(&worker, &parent.id).await;
            let pending = store.get_sandbox(&parent.id).await.unwrap().unwrap();
            assert_eq!(pending.work.step, SandboxWorkStep::StopAgents);
            assert!(pending.runtime.is_some());
            f.backend.deny_inventory.store(false, Ordering::SeqCst);
            composed::run_due(&worker, &parent.id).await;
            let pending = store.get_sandbox(&parent.id).await.unwrap().unwrap();
            assert_eq!(
                pending.work.step,
                SandboxWorkStep::RevokeRetirementCredentials
            );
            f.harness.credentials.fail_revokes(1);
            composed::run_due(&worker, &parent.id).await;
            let pending = store.get_sandbox(&parent.id).await.unwrap().unwrap();
            assert_eq!(
                pending.work.step,
                SandboxWorkStep::RevokeRetirementCredentials
            );
            assert!(pending.runtime.is_some());
            assert!(!pending.lifecycle.home.as_ref().unwrap().released);
            composed::run_due(&worker, &parent.id).await;

            f.backend.deny_teardown.store(true, Ordering::SeqCst);
            composed::run_due(&worker, &parent.id).await;
            let pending = store.get_sandbox(&parent.id).await.unwrap().unwrap();
            assert_eq!(pending.work.step, SandboxWorkStep::TeardownRuntime);
            assert!(pending.runtime.is_some());
            assert!(!pending.lifecycle.home.as_ref().unwrap().released);
            f.backend.deny_teardown.store(false, Ordering::SeqCst);
        }

        for _ in 0..8 {
            if store.get_sandbox(&parent.id).await.unwrap().unwrap().status
                == SandboxStatus::Retired
            {
                break;
            }
            composed::run_due(&worker, &parent.id).await;
        }
        let retired = store.get_sandbox(&parent.id).await.unwrap().unwrap();
        assert_eq!(
            retired.status,
            SandboxStatus::Retired,
            "capture={capture}, custody={custody}"
        );
        assert!(retired.runtime.is_none());
        assert!(retired.lifecycle.home.unwrap().released);
        assert_eq!(
            store.get_agent(&child.id).await.unwrap().unwrap().status,
            AgentStatus::Deleted
        );
        assert!(store.action(&parent.id).await.unwrap().is_none());
        let row = client
            .database(&name)
            .collection::<Document>("sandboxes")
            .find_one(doc! {"_id": parent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key(CAPTURE));
        assert_eq!(receiver.calls.load(Ordering::SeqCst) > 0, custody);
        receiver.server.abort();
        client.database(&name).drop().await.unwrap();
    }
}
