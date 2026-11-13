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
