use super::*;
use layer2_core::resource_access::{ResourceAccessStore, ResourceCapability};

mod budget;
mod effect_budget;
mod effects;
mod fixture;
mod http;
mod process;
mod process_compatibility;
mod public_status;
mod supervision;
mod sweep;
use fixture::Fixture;

fn incomplete(progress: Progress, completed: u32, total: u32) {
    let Progress::Pending {
        completed: observed,
        total: frozen,
        failures,
        ..
    } = progress
    else {
        panic!("incomplete participant evidence cannot finalize")
    };
    assert_eq!((observed, frozen), (completed, total));
    assert!(!failures.is_empty());
}

async fn final_operation(
    fixture: &Fixture,
    original: &RevocationOperationV1,
) -> RevocationOperationV1 {
    let stored = fixture
        .store
        .revocation(&original.key.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.key, original.key);
    assert!(stored.intent == original.intent);
    assert_eq!(stored.frozen_members, original.frozen_members);
    assert_eq!(stored.deadline_unix_ms, original.deadline_unix_ms);
    let RevocationPhaseV1::Effective { receipt, .. } = &stored.phase else {
        panic!()
    };
    assert_eq!(receipt.operation_id, original.key.operation_id.as_str());
    assert_eq!(receipt.share.revision, fixture.share.revision + 1);
    assert!(receipt.share.revoked_at.is_some());
    let snapshot = fixture
        .sandbox
        .resource_access_snapshot(&fixture.target)
        .await
        .unwrap();
    assert_eq!(snapshot.fence().policy_revision, receipt.policy_revision);
    stored
}

// Participant endpoints are synthetic. These contracts exercise the real worker,
// coordinator and Mongo transactions, not native stop or HTTP transport finality.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn lost_stop_reply_and_reopened_store_preserve_frozen_identity() {
    for lost_ordinal in [1, 2] {
        let fixture = Fixture::new().await;
        let rows = vec![
            fixture.register("command").await,
            fixture.release("response").await,
        ];
        let directory = Directory::new(&rows);
        directory
            .endpoint(lost_ordinal)
            .behavior
            .lock()
            .unwrap()
            .lose_reply = true;
        let operation = fixture.freeze().await;
        let worker = RevocationWorker::new(fixture.store.clone(), directory.clone());
        assert_eq!(
            worker.discover(None).await.unwrap().operations.as_slice(),
            std::slice::from_ref(&operation.key.operation_id)
        );
        incomplete(
            worker.reconcile(&operation.key.operation_id).await.unwrap(),
            1,
            2,
        );
        let lost = fixture
            .store
            .record(&rows[(lost_ordinal - 1) as usize].registration)
            .await
            .unwrap()
            .unwrap();
        assert!(lost.barrier.is_some());
        assert!(lost.completion.is_none());
        assert!(matches!(
            fixture
                .store
                .revocation(&operation.key.operation_id)
                .await
                .unwrap()
                .unwrap()
                .phase,
            RevocationPhaseV1::Pending
        ));
        drop(worker);
        let restarted = RevocationWorker::new(fixture.reopened().await, directory.clone());
        assert_eq!(
            restarted
                .reconcile(&operation.key.operation_id)
                .await
                .unwrap(),
            Progress::Effective
        );
        assert_eq!(
            *directory.endpoint(lost_ordinal).calls.lock().unwrap(),
            ["query", "barrier", "stop", "query"]
        );
        let final_state = final_operation(&fixture, &operation).await;
        assert!(fixture.freeze().await == final_state);
        assert!(restarted
            .discover(None)
            .await
            .unwrap()
            .operations
            .is_empty());
        for row in rows {
            let retained = fixture
                .store
                .record(&row.registration)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(retained.member, row.member);
            assert_eq!(retained.basis, row.basis);
            assert!(retained.completion.is_some());
        }
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn two_workers_converge_on_one_exact_share_receipt() {
    let fixture = Fixture::new().await;
    let rows = vec![
        fixture.register("concurrent").await,
        fixture.release("response").await,
    ];
    let directory = Directory::new(&rows);
    *directory.endpoint(1).query_gate.lock().unwrap() =
        Some(Arc::new(tokio::sync::Barrier::new(2)));
    let operation = fixture.freeze().await;
    let first = RevocationWorker::new(fixture.store.clone(), directory.clone());
    let second = RevocationWorker::new(fixture.reopened().await, directory.clone());
    let (a, b) = tokio::join!(
        first.reconcile(&operation.key.operation_id),
        second.reconcile(&operation.key.operation_id)
    );
    assert_eq!(a.unwrap(), Progress::Effective);
    assert_eq!(b.unwrap(), Progress::Effective);
    let final_state = final_operation(&fixture, &operation).await;
    assert!(fixture.freeze().await == final_state);
    let rows = fixture
        .store
        .frozen_members(&operation.key.operation_id, 0, MAX_PAGE_MEMBERS)
        .await
        .unwrap();
    final_state.validate_cohort(&rows).unwrap();
    let calls: Vec<_> = directory
        .0
        .values()
        .map(|endpoint| endpoint.calls.lock().unwrap().clone())
        .collect();
    assert_eq!(
        first.reconcile(&operation.key.operation_id).await.unwrap(),
        Progress::Effective
    );
    let after: Vec<_> = directory
        .0
        .values()
        .map(|endpoint| endpoint.calls.lock().unwrap().clone())
        .collect();
    assert_eq!(after, calls);
    assert!(
        fixture
            .store
            .finish_revocation(&operation.key.operation_id)
            .await
            .unwrap()
            == final_state
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn incomplete_quiescence_blocks_finalization_and_late_shared_registration() {
    let fixture = Fixture::new().await;
    let held = fixture
        .authority("recipient", ResourceCapability::Interact)
        .await;
    let rows = vec![
        fixture.register("writer").await,
        fixture.release("transport").await,
        fixture.register("settled").await,
    ];
    let directory = Directory::new(&rows);
    directory.endpoint(1).behavior.lock().unwrap().local_writer = true;
    directory
        .endpoint(2)
        .behavior
        .lock()
        .unwrap()
        .transport_open = true;
    let operation = fixture.freeze().await;
    let worker = RevocationWorker::new(fixture.store.clone(), directory.clone());
    incomplete(
        worker.reconcile(&operation.key.operation_id).await.unwrap(),
        1,
        3,
    );
    assert!(fixture
        .store
        .finish_revocation(&operation.key.operation_id)
        .await
        .is_err());
    let late = fixture.execution("late");
    assert!(matches!(
        fixture.coordinator.register_execution(&held, &late).await,
        Err(ExecutionError::Closing { .. })
    ));
    assert!(fixture
        .store
        .record(&registration_id(&wire::MemberKey::Execution { execution: late }).unwrap())
        .await
        .unwrap()
        .is_none());
    let owner = fixture
        .authority("owner", ResourceCapability::Interact)
        .await;
    fixture
        .coordinator
        .register_execution(&owner, &fixture.execution("owner-independent"))
        .await
        .unwrap();
    assert!(owner.basis().family().is_none());
    let snapshot = fixture
        .sandbox
        .resource_access_snapshot(&fixture.target)
        .await
        .unwrap();
    let share = snapshot
        .shares
        .iter()
        .find(|share| share.id == fixture.share.id)
        .unwrap();
    assert_eq!(share.revision, fixture.share.revision);
    assert!(share.revoked_at.is_none());
    directory.endpoint(1).behavior.lock().unwrap().local_writer = false;
    directory
        .endpoint(2)
        .behavior
        .lock()
        .unwrap()
        .transport_open = false;
    for ordinal in [1, 2] {
        *directory.endpoint(ordinal).retained.lock().unwrap() = None;
    }
    let restarted = RevocationWorker::new(fixture.reopened().await, directory.clone());
    assert_eq!(
        restarted
            .reconcile(&operation.key.operation_id)
            .await
            .unwrap(),
        Progress::Effective
    );
    final_operation(&fixture, &operation).await;
    assert_eq!(
        *directory.endpoint(3).calls.lock().unwrap(),
        ["query", "barrier", "stop"]
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn ambiguous_barrier_and_completion_commits_recover_without_replaying_stop() {
    for completion_commit in [false, true] {
        let fixture = Fixture::new().await;
        let row = fixture.register("commit-cut").await;
        let directory = Directory::new(std::slice::from_ref(&row));
        let operation = fixture.freeze().await;
        if completion_commit {
            let endpoint = directory.endpoint(1);
            retain(endpoint, &operation);
            fixture
                .store
                .observe_barrier(&endpoint.barrier(&row.barrier_request(&operation).unwrap()))
                .await
                .unwrap();
        }
        let worker = RevocationWorker::new(fixture.store.clone(), directory.clone());
        fixture.ambiguous_commits(true).await;
        let progress = worker.reconcile(&operation.key.operation_id).await;
        fixture.ambiguous_commits(false).await;
        incomplete(progress.unwrap(), 0, 1);
        assert!(matches!(
            fixture
                .store
                .revocation(&operation.key.operation_id)
                .await
                .unwrap()
                .unwrap()
                .phase,
            RevocationPhaseV1::Pending
        ));
        let restarted = RevocationWorker::new(fixture.reopened().await, directory.clone());
        assert_eq!(
            restarted
                .reconcile(&operation.key.operation_id)
                .await
                .unwrap(),
            Progress::Effective
        );
        final_operation(&fixture, &operation).await;
        assert_eq!(
            directory
                .endpoint(1)
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|&&call| call == "stop")
                .count(),
            usize::from(!completion_commit)
        );
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn ambiguous_finalization_is_recovered_from_the_original_operation() {
    let fixture = Fixture::new().await;
    let operation = fixture.freeze().await;
    let directory = Directory::new(&[]);
    let worker = RevocationWorker::new(fixture.store.clone(), directory.clone());
    fixture.ambiguous_commits(true).await;
    let progress = worker.reconcile(&operation.key.operation_id).await;
    fixture.ambiguous_commits(false).await;
    assert_eq!(progress, Err(ExecutionError::Unknown));
    let restarted = RevocationWorker::new(fixture.reopened().await, directory);
    assert_eq!(
        restarted
            .reconcile(&operation.key.operation_id)
            .await
            .unwrap(),
        Progress::Effective
    );
    let final_state = final_operation(&fixture, &operation).await;
    assert!(fixture.freeze().await == final_state);
    assert!(restarted
        .discover(None)
        .await
        .unwrap()
        .operations
        .is_empty());
    fixture.cleanup().await;
}
