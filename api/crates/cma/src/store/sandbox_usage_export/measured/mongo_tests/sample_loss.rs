use super::refresh::{assert_hold, head, retry};
use super::support_loss::publish;
use super::*;
use layer2_core::sandbox_usage_evidence::MeasuredQuality;

async fn early(f: &Fixture) -> PhysicalSeries {
    f.bind().await;
    let mut samples = f.samples(30.0).remove(0);
    samples.samples[1].at = (f.minute + 30) as f64;
    let (published, original) = publish(f, vec![samples.clone()]).await;
    assert!(published);
    let cpu = &original.manifest.frozen.as_ref().unwrap().fact().metrics[0];
    assert_eq!(cpu.quantity.as_deref(), Some("30000000000"));
    assert_eq!(cpu.covered_binding_ms, "30000");
    assert_eq!(cpu.quality, MeasuredQuality::Partial);
    samples
}

async fn shifted_samples(reset: bool) {
    let f = Fixture::new().await;
    let mut samples = early(&f).await;
    let before = head(&f).await;
    let outbox = f.rows(OUTBOX).await;
    let retained = f.rows(EVIDENCE).await;
    if reset {
        samples.samples[0].value = Some(50.0);
    } else {
        samples.samples.remove(0);
    }
    samples.samples.push(Sample {
        at: (f.minute + 60) as f64,
        value: Some(55.0),
    });
    let (published, candidate) = publish(&f, vec![samples]).await;
    let cpu = &candidate.manifest.frozen.as_ref().unwrap().fact().metrics[0];
    assert_eq!(cpu.quantity.as_deref(), Some("15000000000"));
    assert_eq!(cpu.covered_binding_ms, "30000");
    assert_eq!(cpu.expected_binding_ms, "60000");
    assert_eq!(cpu.quality, MeasuredQuality::Partial);
    assert!(!published);
    assert_hold(&f, "refresh_evidence_incomplete", &before, &outbox).await;
    let after = f.rows(EVIDENCE).await;
    assert!(retained.iter().all(|row| after.contains(row)));
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn equal_sample_coverage_on_a_different_interval_cannot_erase_retained_usage() {
    shifted_samples(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn counter_reset_cannot_move_support_to_another_interval() {
    shifted_samples(true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn late_samples_that_preserve_prior_interval_can_complete_the_fact() {
    let f = Fixture::new().await;
    let mut samples = early(&f).await;
    samples.samples.push(Sample {
        at: (f.minute + 60) as f64,
        value: Some(55.0),
    });
    let (published, candidate) = publish(&f, vec![samples]).await;
    let cpu = &candidate.manifest.frozen.as_ref().unwrap().fact().metrics[0];
    assert_eq!(cpu.quantity.as_deref(), Some("45000000000"));
    assert_eq!(cpu.covered_binding_ms, "60000");
    assert_eq!(cpu.quality, MeasuredQuality::Observed);
    assert!(published);
    let events = f.events().await;
    assert_eq!(events.len(), 3);
    assert_eq!(events[2]["kind"], "correction");
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn supported_partial_correction_can_decrease_to_zero() {
    let f = Fixture::new().await;
    let mut samples = early(&f).await;
    for (last, quantity) in [(25.0, "15000000000"), (10.0, "0")] {
        samples.samples[1].value = Some(last);
        retry(&f).await;
        let (published, candidate) = publish(&f, vec![samples.clone()]).await;
        let cpu = &candidate.manifest.frozen.as_ref().unwrap().fact().metrics[0];
        assert_eq!(cpu.quantity.as_deref(), Some(quantity));
        assert_eq!(cpu.covered_binding_ms, "30000");
        assert_eq!(cpu.quality, MeasuredQuality::Partial);
        assert!(published);
    }
    assert_eq!(f.events().await.len(), 4);
    f.db.drop().await.unwrap();
}
