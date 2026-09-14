use super::*;

fn replica(harness: &Harness) -> Arc<dyn SandboxAdapter> {
    let mut spec = CodexSandboxSpec::default();
    spec.deployment.image.uri = "registry.example/runtime:next".into();
    Arc::new(CodexAdapter::with_backend(harness.backend.clone(), spec))
}

#[tokio::test]
async fn another_replica_installs_a_new_binding_without_waiting_for_inventory_reconciliation() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let adapter = replica(&harness);
    assert!(adapter.runtime_inventory().await.unwrap().is_empty());
    harness.run(&created.id, now).await;
    let binding = harness.stored(&created.id).await.runtime.unwrap();
    let worker = harness.worker_with_adapter("another-replica", adapter.clone());
    for _ in 0..4 {
        assert!(worker.run_one(&created.id, now).await.unwrap());
    }
    let active = harness.stored(&created.id).await;
    assert_eq!(active.status, SandboxStatus::Active);
    assert_eq!(active.runtime, Some(binding));
    assert_eq!(adapter.runtime_inventory().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_missing_bound_runtime_does_not_allocate_or_fail_installation() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    harness.run(&created.id, now).await;
    let installing = harness.stored(&created.id).await;
    let handle = RuntimeHandle(installing.runtime.as_ref().unwrap().runtime_id.clone());
    harness.adapter.teardown(&handle).await.unwrap();
    let adapter = replica(&harness);
    let worker = harness.worker_with_adapter("another-replica", adapter.clone());
    assert!(worker.run_one(&created.id, now).await.unwrap());
    let retry = harness.stored(&created.id).await;
    assert_eq!(retry.status, SandboxStatus::Installing);
    assert_eq!(retry.work.step, SandboxWorkStep::RunInstall);
    assert_eq!(retry.runtime, installing.runtime);
    assert!(retry.failure.is_none());
    assert!(adapter.runtime_inventory().await.unwrap().is_empty());
}

#[tokio::test]
async fn another_replica_retires_a_runtime_even_after_it_has_disappeared() {
    let harness = Harness::new(Vec::new(), 0, Duration::from_secs(60)).await;
    let created = harness.create().await;
    let now = start_time(&created);
    let active = harness.drive_active(&created.id, now).await;
    let handle = RuntimeHandle(active.runtime.as_ref().unwrap().runtime_id.clone());
    let revision = active.revision;
    let mut retiring = active.clone();
    retiring.status = SandboxStatus::Retiring;
    retiring
        .work
        .schedule(SandboxWorkStep::TeardownRuntime, rfc3339_at(now));
    retiring.revision += 1;
    harness
        .store
        .save_sandbox(revision, retiring)
        .await
        .unwrap();
    harness.adapter.teardown(&handle).await.unwrap();
    let adapter = replica(&harness);
    let worker = harness.worker_with_adapter("another-replica", adapter.clone());
    assert!(worker.run_one(&created.id, now).await.unwrap());
    let retired = harness.stored(&created.id).await;
    assert_eq!(retired.work.step, SandboxWorkStep::MarkRetired);
    assert!(retired.runtime.is_none());
    assert!(adapter.runtime_inventory().await.unwrap().is_empty());
}
