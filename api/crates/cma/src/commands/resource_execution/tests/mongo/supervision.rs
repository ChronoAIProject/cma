use super::super::super::sweep::RevocationSweep;
use super::*;
use std::time::Duration;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn supervised_loop_failure_keeps_durable_barrier_and_owned_finalization() {
    let fixture = Fixture::new().await;
    let row = fixture.register("supervised-recovery").await;
    let directory = Directory::new(std::slice::from_ref(&row));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    *directory.endpoint(1).stop_gate.lock().unwrap() = Some(gate.clone());
    let original = fixture.freeze().await;
    let activity = crate::worker_supervision::Workers::new().get("agent_commands");
    let (stop, signal) = tokio::sync::watch::channel(false);
    let supervisor = RevocationSweep::new(Arc::new(RevocationWorker::new(
        fixture.store.clone(),
        directory.clone(),
    )))
    .spawn_supervised(signal, activity.clone());
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = fixture
                .store
                .record(&row.registration)
                .await
                .unwrap()
                .unwrap();
            assert!(record.completion.is_none());
            if record.barrier.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    crate::worker_supervision::panic_next_poll(&activity);
    tokio::time::timeout(Duration::from_secs(2), async {
        while activity.snapshot().state != "backoff" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        *directory.endpoint(1).calls.lock().unwrap(),
        ["query", "barrier"]
    );
    stop.send(true).unwrap();
    gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), supervisor)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(activity.snapshot().state, "stopped");
    let reopened = fixture.reopened().await;
    let completed = reopened
        .revocation(&original.key.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert!(completed == final_operation(&fixture, &original).await);
    assert!(fixture.freeze().await == completed);
    assert_eq!(
        *directory.endpoint(1).calls.lock().unwrap(),
        ["query", "barrier", "stop"]
    );
    fixture.cleanup().await;
}
