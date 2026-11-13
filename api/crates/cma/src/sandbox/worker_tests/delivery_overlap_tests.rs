use super::*;

#[path = "delivery_overlap_tests/fixtures.rs"]
mod fixtures;
use fixtures::*;

// Exercise the real worker, adapter proof, credential validation and publication.
// Only dependency latency is synthetic; this is not a desktop/client SLO probe.
#[tokio::test]
async fn credential_delivery_phase_measurement() {
    let mut samples = Vec::new();
    for _ in 0..20 {
        let f = Fixture::new(Duration::from_millis(120), Duration::from_millis(80)).await;
        let started = tokio::time::Instant::now();
        f.worker.run_one(&f.id, f.now).await.unwrap();
        samples.push(started.elapsed().as_micros() as u64);
        assert_eq!(
            f.harness.stored(&f.id).await.work.step,
            SandboxWorkStep::Activate
        );
        assert_eq!(f.backend.deliveries.load(Ordering::SeqCst), 1);
        assert_eq!(f.credentials.started.load(Ordering::SeqCst), 1);
        assert_eq!(f.backend.inspections.load(Ordering::SeqCst), 1);
    }
    samples.sort_unstable();
    println!(
        "credential_delivery_phase n=20 unit=us p50={} p95={} max={} samples={samples:?}",
        samples[9], samples[18], samples[19]
    );
}

#[tokio::test(start_paused = true)]
async fn read_latency_overlaps_without_skipping_inspection_or_delivery() {
    let f = Fixture::new(Duration::from_millis(120), Duration::from_millis(80)).await;
    let started = tokio::time::Instant::now();
    f.worker.run_one(&f.id, f.now).await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(150));
    assert_eq!(f.backend.inspections.load(Ordering::SeqCst), 1);
    assert_eq!(f.backend.deliveries.load(Ordering::SeqCst), 1);
    assert!(f.harness.stored(&f.id).await.credential.delivered);
}

#[tokio::test(start_paused = true)]
async fn inspection_refusal_wins_over_material_failure_or_pending_read() {
    for delay in [Duration::ZERO, Duration::from_secs(3600)] {
        let f = Fixture::new(Duration::from_millis(120), delay).await;
        *f.backend.failure.lock().unwrap() = Some(layer3_adapter::AdapterError::Rejected(
            "inspection refused".into(),
        ));
        *f.credentials.failure.lock().unwrap() =
            Some(CredentialError::Unavailable("custody unavailable".into()));
        let started = tokio::time::Instant::now();
        f.worker.run_one(&f.id, f.now).await.unwrap();
        assert!(started.elapsed() < Duration::from_millis(150));
        assert_eq!(f.credentials.started.load(Ordering::SeqCst), 1);
        assert_eq!(f.credentials.active.load(Ordering::SeqCst), 0);
        assert_eq!(f.backend.deliveries.load(Ordering::SeqCst), 0);
        let stored = f.harness.stored(&f.id).await;
        assert_eq!(stored.status, SandboxStatus::Failed);
        assert!(!stored.credential.delivered);
    }
}

#[tokio::test(start_paused = true)]
async fn material_failure_waits_for_proof_then_retries_without_delivery() {
    let f = Fixture::new(Duration::from_millis(120), Duration::ZERO).await;
    *f.credentials.failure.lock().unwrap() =
        Some(CredentialError::Unavailable("custody unavailable".into()));
    let started = tokio::time::Instant::now();
    f.worker.run_one(&f.id, f.now).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(120));
    assert_eq!(f.backend.deliveries.load(Ordering::SeqCst), 0);
    let stored = f.harness.stored(&f.id).await;
    assert_eq!(stored.work.step, SandboxWorkStep::DeliverInitialCredential);
    assert_eq!(stored.work.attempts, 1);
}

#[tokio::test(start_paused = true)]
async fn cancellation_drops_both_reads_without_delivery_or_publication() {
    let f = Fixture::new(Duration::from_secs(60), Duration::from_secs(60)).await;
    let worker = f.worker.clone();
    let id = f.id.clone();
    let now = f.now;
    let task = tokio::spawn(async move { worker.run_one(&id, now).await });
    tokio::task::yield_now().await;
    assert_eq!(f.backend.active.load(Ordering::SeqCst), 1);
    assert_eq!(f.credentials.active.load(Ordering::SeqCst), 1);
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(f.backend.active.load(Ordering::SeqCst), 0);
    assert_eq!(f.credentials.active.load(Ordering::SeqCst), 0);
    assert_eq!(f.backend.deliveries.load(Ordering::SeqCst), 0);
    assert!(!f.harness.stored(&f.id).await.credential.delivered);
}

#[tokio::test(start_paused = true)]
async fn consumption_rechecks_expiry_after_all_dependency_waits() {
    let f = Fixture::new(Duration::from_secs(2), Duration::from_millis(80)).await;
    let mut stored = f.harness.stored(&f.id).await;
    let mut material = f
        .harness
        .credentials
        .material(&f.id, 1)
        .await
        .unwrap()
        .unwrap();
    material.authority.expires_at =
        rfc3339_at(f.now + CREDENTIAL_EXPIRY_MARGIN + Duration::from_secs(1));
    stored.credential.pending = Some(material.authority.clone());
    stored.revision += 1;
    f.harness
        .store
        .save_sandbox(stored.revision - 1, stored)
        .await
        .unwrap();
    f.harness.credentials.restore(material);
    f.worker.run_one(&f.id, f.now).await.unwrap();
    assert_eq!(f.backend.deliveries.load(Ordering::SeqCst), 0);
    assert_eq!(f.harness.stored(&f.id).await.status, SandboxStatus::Failed);
}

#[tokio::test(start_paused = true)]
async fn fetched_material_must_match_exact_claimed_custody() {
    for change in [0, 1, 2] {
        let f = Fixture::new(Duration::from_millis(120), Duration::from_millis(80)).await;
        let mut material = f
            .harness
            .credentials
            .material(&f.id, 1)
            .await
            .unwrap()
            .unwrap();
        match change {
            0 => material.owner = SubjectId("different-owner".into()),
            1 => material.authority.provider_state_version += 1,
            _ => material.authority.grant_fingerprint = "f".repeat(64),
        }
        f.harness.credentials.restore(material);
        f.worker.run_one(&f.id, f.now).await.unwrap();
        assert_eq!(f.backend.deliveries.load(Ordering::SeqCst), 0);
        assert_eq!(f.harness.stored(&f.id).await.status, SandboxStatus::Failed);
    }
}
