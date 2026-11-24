use super::*;
use crate::sandbox_metrics::tests::binding;
use layer3_adapter::sandbox_metrics::{decode_raw_export, Selection};

fn series(binding: &Binding, metric: SourceMetric, samples: &[(u64, f64)]) -> RawPhysicalSeries {
    exported_series(binding, metric, samples, None)
}

fn exported_series(
    binding: &Binding,
    metric: SourceMetric,
    samples: &[(u64, f64)],
    omit: Option<&str>,
) -> RawPhysicalSeries {
    let mut labels = BTreeMap::<String, String>::from([
        ("cluster".into(), binding.cluster.clone()),
        ("namespace".into(), binding.namespace.clone()),
        ("__name__".into(), metric.name().into()),
        ("job".into(), "kubelet".into()),
        ("metrics_path".into(), metric.metrics_path().into()),
    ]);
    let name = if metric.is_pvc() {
        labels.insert("pvc_uid".into(), binding.pvc_uid.clone());
        binding.pvc_name.clone()
    } else {
        labels.insert("container".into(), "runtime".into());
        labels.insert("pod_uid".into(), binding.pod_uid.clone().unwrap());
        labels.insert(
            "container_id".into(),
            binding.runtime_container_id.clone().unwrap(),
        );
        binding.pod_name.clone().unwrap()
    };
    labels.insert(metric.identity_label().into(), name.clone());
    if let Some(key) = omit {
        labels.remove(key);
    }
    let (timestamps, values): (Vec<_>, Vec<_>) = samples.iter().copied().unzip();
    let row = serde_json::json!({"metric": labels, "timestamps": timestamps, "values": values});
    decode_raw_export(
        row.to_string().as_bytes(),
        &Selection {
            cluster: binding.cluster.clone(),
            namespace: binding.namespace.clone(),
            names: vec![name],
            metric,
            from: 0,
            to: 600,
        },
    )
    .unwrap()
    .pop()
    .unwrap()
}

const WINDOW: Range = Range { from: 10, to: 50 };

fn capture(
    binding: &Binding,
    metric: SourceMetric,
    source: RawPhysicalSeries,
) -> MeasuredObservation {
    capture_observations(binding, 1, metric, &[source], WINDOW)
        .unwrap()
        .pop()
        .unwrap()
}

#[test]
fn split_out_of_order_samples_replay_and_open_bindings_end_at_the_observation() {
    let mut b = binding(BindingKind::Compute);
    b.valid_from = 20;
    b.valid_until = None;
    b.observed_at = 40;
    let metric = SourceMetric::CpuCounter;
    let left = series(&b, metric, &[(39_999, 6.125), (20_000, 2.0), (40_000, 7.0)]);
    let right = series(&b, metric, &[(39_999, 6.125), (19_999, 1.0)]);
    let forward =
        capture_observations(&b, 1, metric, &[left.clone(), right.clone()], WINDOW).unwrap();
    let reverse = capture_observations(&b, 1, metric, &[right, left], WINDOW).unwrap();
    assert_eq!(forward, reverse);
    assert_eq!(
        forward
            .iter()
            .map(|sample| sample.timestamp_ms)
            .collect::<Vec<_>>(),
        [20_000, 39_999]
    );
    assert_eq!(forward[1].source_value_be, 6.125_f64.to_be_bytes());

    let narrower = capture_observations(
        &b,
        2,
        metric,
        &[series(&b, metric, &[(39_999, 6.125)])],
        Range { from: 30, to: 45 },
    )
    .unwrap();
    assert_eq!(
        forward[1].physical_sample_id,
        narrower[0].physical_sample_id
    );
    assert_ne!(
        forward[1].evidence_sha256().unwrap(),
        narrower[0].evidence_sha256().unwrap()
    );
    assert_eq!(forward[1].verify_replay(&narrower[0]), Ok(()));
}

#[test]
fn rejects_missing_uid_and_conflicting_sample_values() {
    let b = binding(BindingKind::Compute);
    let metric = SourceMetric::WorkingSet;
    let source = series(&b, metric, &[(30_001, 1024.0)]);
    for label in ["pod_uid", "container_id"] {
        let invalid = exported_series(&b, metric, &[(30_001, 1024.0)], Some(label));
        assert_eq!(
            capture_observations(&b, 1, metric, &[invalid], WINDOW),
            Err(EvidenceError::Attribution)
        );
    }
    let other = series(&b, metric, &[(30_001, 2048.0)]);
    assert_eq!(
        capture_observations(&b, 1, metric, &[source, other], WINDOW),
        Err(EvidenceError::ConflictingObservation)
    );
}

#[test]
fn split_batches_with_different_value_bits_cannot_disagree_about_replay() {
    let b = binding(BindingKind::Compute);
    let metric = SourceMetric::CpuCounter;
    let positive = series(&b, metric, &[(30_000, 0.0)]);
    let negative = series(&b, metric, &[(30_000, -0.0)]);
    let first = capture(&b, metric, positive.clone());
    let next = capture(&b, metric, negative.clone());
    assert_eq!(first.physical_sample_id(), next.physical_sample_id());
    assert_eq!(
        first.verify_replay(&next),
        Err(EvidenceError::ConflictingObservation)
    );
    assert_eq!(
        next.verify_replay(&first),
        Err(EvidenceError::ConflictingObservation)
    );
    for input in [[positive.clone(), negative.clone()], [negative, positive]] {
        assert_eq!(
            capture_observations(&b, 1, metric, &input, WINDOW),
            Err(EvidenceError::ConflictingObservation)
        );
    }
}

#[test]
fn container_restart_and_pod_reuse_create_distinct_physical_samples() {
    let mut b = binding(BindingKind::Compute);
    let metric = SourceMetric::CpuCounter;
    let original = series(&b, metric, &[(30_000, 10.0)]);
    let before = capture(&b, metric, original.clone());
    b.runtime_container_id = Some("container-2".into());
    b.restart_count = Some(1);
    assert_eq!(
        capture_observations(&b, 1, metric, &[original], WINDOW),
        Err(EvidenceError::Attribution)
    );
    let restarted = capture(&b, metric, series(&b, metric, &[(30_000, 3.0)]));
    assert_ne!(before.physical_sample_id, restarted.physical_sample_id);
    b.pod_uid = Some("replacement-pod".into());
    let reused_name = capture(&b, metric, series(&b, metric, &[(30_000, 3.0)]));
    assert_ne!(restarted.physical_sample_id, reused_name.physical_sample_id);
}

#[test]
fn shared_home_replays_across_generations_but_conflicting_attribution_refuses() {
    let mut b = binding(BindingKind::Home);
    let metric = SourceMetric::PvcUsed;
    let source = series(&b, metric, &[(30_000, 1024.0)]);
    let first = capture(&b, metric, source.clone());
    b.runtime_generation = Some(2);
    b.revision = 2;
    let replay = capture(&b, metric, source.clone());
    assert_eq!(first.physical_sample_id, replay.physical_sample_id);
    assert_eq!(first.verify_replay(&replay), Ok(()));

    b.owner_subject = "other-owner".into();
    let disputed = capture(&b, metric, source.clone());
    assert_eq!(first.physical_sample_id, disputed.physical_sample_id);
    assert_eq!(
        first.verify_replay(&disputed),
        Err(EvidenceError::ConflictingObservation)
    );
    b.pvc_uid = "replacement-volume".into();
    assert_eq!(
        capture_observations(&b, 1, metric, &[source], WINDOW),
        Err(EvidenceError::Attribution)
    );
}
