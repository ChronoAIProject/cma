use super::*;

#[tokio::test]
async fn stale_mint_worker_is_fenced_after_lease_takeover() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    harness.run(&created.id, now).await;
    harness.run(&created.id, now).await;
    assert_eq!(
        harness.stored(&created.id).await.work.step,
        SandboxWorkStep::MintInitialCredential
    );

    let blocking = BlockingCredentials::new(Arc::clone(&harness.credentials));
    let credentials: Arc<dyn SandboxCredentialController> = blocking.clone();
    let worker_a = harness.worker_with(
        "stale-worker",
        Arc::clone(&credentials),
        Duration::from_secs(1),
    );
    let worker_b = harness.worker_with("takeover-worker", credentials, Duration::from_secs(1));
    let entered = blocking.entered.notified();
    let id = created.id.clone();
    let stale = tokio::spawn(async move { worker_a.run_one(&id, now).await });
    entered.await;

    assert!(worker_b
        .run_one(&created.id, now + Duration::from_secs(2))
        .await
        .unwrap());
    assert_eq!(
        harness.stored(&created.id).await.work.step,
        SandboxWorkStep::DeliverInitialCredential
    );
    blocking.release.notify_one();

    assert_eq!(stale.await.unwrap(), Err(SandboxStoreError::Conflict));
    assert_eq!(
        harness.stored(&created.id).await.work.step,
        SandboxWorkStep::DeliverInitialCredential
    );
}
