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
            .map(|sample| sample.0.timestamp_ms)
            .collect::<Vec<_>>(),
        [20_000, 39_999]
    );
    assert_eq!(forward[1].0.source_value_be, 6.125_f64.to_be_bytes());

    let narrower = capture_observations(
        &b,
        2,
        metric,
        &[series(&b, metric, &[(39_999, 6.125)])],
        Range { from: 30, to: 45 },
    )
    .unwrap();
    assert_eq!(
        forward[1].physical_sample_id(),
        narrower[0].physical_sample_id()
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
    assert_ne!(before.physical_sample_id(), restarted.physical_sample_id());
    b.pod_uid = Some("replacement-pod".into());
    let reused_name = capture(&b, metric, series(&b, metric, &[(30_000, 3.0)]));
    assert_ne!(
        restarted.physical_sample_id(),
        reused_name.physical_sample_id()
    );
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
    assert_eq!(first.physical_sample_id(), replay.physical_sample_id());
    assert_eq!(first.verify_replay(&replay), Ok(()));

    b.owner_subject = "other-owner".into();
    let disputed = capture(&b, metric, source.clone());
    assert_eq!(first.physical_sample_id(), disputed.physical_sample_id());
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

#[test]
fn retained_payload_checks_both_digests_and_preserves_replay_semantics() {
    let mut b = binding(BindingKind::Compute);
    let metric = SourceMetric::CpuCounter;
    let physical = series(&b, metric, &[(30_001, 3.0)]);
    let original = capture(&b, metric, physical.clone());
    let encoded = serde_json::to_vec(&original).unwrap();
    let id = original.physical_sample_id();
    let digest = original.evidence_sha256().unwrap();
    let confirmation = RetainedObservation::decode(&encoded, id, &digest).unwrap();
    let replay = capture_observations(&b, 2, metric, std::slice::from_ref(&physical), WINDOW)
        .unwrap()
        .remove(0);
    assert_eq!(confirmation.verify_replay(&replay), Ok(()));

    b.owner_subject = "other-owner".into();
    let dispute = capture(&b, metric, physical);
    let conflicting = RetainedObservation::decode(
        &serde_json::to_vec(&dispute).unwrap(),
        id,
        &dispute.evidence_sha256().unwrap(),
    )
    .unwrap();
    assert_eq!(
        confirmation.verify_retained_replay(&conflicting),
        Err(EvidenceError::ConflictingObservation),
    );
    assert!(RetainedObservation::decode(&encoded, "foreign-id", &digest).is_err());
    assert!(RetainedObservation::decode(&encoded, id, "foreign-digest").is_err());
    let mut corrupt: ObservationV1 = serde_json::from_slice(&encoded).unwrap();
    corrupt.binding.valid_until = Some(corrupt.timestamp_ms / 1000);
    assert!(RetainedObservation::decode(
        &serde_json::to_vec(&corrupt).unwrap(),
        id,
        &super::digest(&corrupt).unwrap(),
    )
    .is_err());
}

mod intervals {
    use super::*;
    use crate::sandbox_metrics::measured_evidence::interval::{
        normalize_series, CpuSeconds, CustodySample, MeasuredInterval, Quantity, Unresolved,
        POLICY_VERSION,
    };

    fn retained(
        binding: &Binding,
        metric: SourceMetric,
        timestamp_ms: u64,
        value: f64,
        mapping_revision: u64,
    ) -> (RetainedObservation, String) {
        let rows = [series(binding, metric, &[(timestamp_ms, value)])];
        let observation = capture_observations(
            binding,
            mapping_revision,
            metric,
            &rows,
            Range { from: 0, to: 600 },
        )
        .unwrap()
        .pop()
        .unwrap();
        let digest = observation.evidence_sha256().unwrap();
        let retained = RetainedObservation::decode(
            &serde_json::to_vec(&observation).unwrap(),
            observation.physical_sample_id(),
            &digest,
        )
        .unwrap();
        (retained, digest)
    }

    fn pair(
        first: &RetainedObservation,
        next: &RetainedObservation,
    ) -> Result<MeasuredInterval, Unresolved> {
        let interval = normalize_series(&[
            CustodySample::Undisputed(first),
            CustodySample::Undisputed(next),
        ])?
        .pop()
        .unwrap();
        interval.quantity?;
        Ok(interval)
    }

    #[test]
    fn subtracts_exact_cpu_values_before_rounding_and_orders_provenance() {
        let binding = binding(BindingKind::Compute);
        let metric = SourceMetric::CpuCounter;
        let (first, first_digest) = retained(&binding, metric, 21_001, 1.0, 2);
        let (next, next_digest) = retained(&binding, metric, 50_000, (1_u64 << 54) as f64, 1);
        let measured = pair(&next, &first).unwrap();
        assert_eq!(measured, pair(&first, &next).unwrap());
        assert_eq!(measured.policy_version, POLICY_VERSION);
        assert_eq!((measured.from_ms, measured.to_ms), (21_001, 50_000));
        assert_eq!(measured.evidence_sha256, [first_digest, next_digest]);
        assert_eq!(
            measured.quantity,
            Ok(Quantity::Cpu(CpuSeconds {
                significand: (1_u128 << 54) - 1,
                exponent: 0,
            }))
        );
    }

    #[test]
    fn bounds_the_canonical_cpu_delta_after_subtraction() {
        let binding = binding(BindingKind::Compute);
        let metric = SourceMetric::CpuCounter;
        let (first, _) = retained(&binding, metric, 21_000, 1.0, 1);
        for (power, significand) in [(76, (1_u128 << 76) - 1), (128, u128::MAX)] {
            let (next, _) = retained(&binding, metric, 30_000, 2_f64.powi(power), 1);
            assert_eq!(
                pair(&first, &next).unwrap().quantity,
                Ok(Quantity::Cpu(CpuSeconds {
                    significand,
                    exponent: 0
                }))
            );
        }
        let (too_large, _) = retained(&binding, metric, 30_000, 2_f64.powi(129), 1);
        assert_eq!(pair(&first, &too_large), Err(Unresolved::Precision));
    }

    #[test]
    fn spacing_limit_and_cpu_reset_leave_only_the_affected_pair_pending() {
        let binding = binding(BindingKind::Compute);
        let metric = SourceMetric::CpuCounter;
        let (first, _) = retained(&binding, metric, 1_000, 3.0, 1);
        let (reset, _) = retained(&binding, metric, 91_000, 2.0, 1);
        let (equal, _) = retained(&binding, metric, 181_000, 2.0, 1);
        let (gap, _) = retained(&binding, metric, 271_001, 9.0, 1);
        let intervals = normalize_series(&[
            CustodySample::Undisputed(&gap),
            CustodySample::Undisputed(&first),
            CustodySample::Undisputed(&equal),
            CustodySample::Undisputed(&reset),
        ])
        .unwrap();
        assert_eq!(intervals.len(), 3);
        assert_eq!(intervals[0].quantity, Err(Unresolved::CounterReset));
        let zero = &intervals[1];
        assert_eq!((zero.from_ms, zero.to_ms), (91_000, 181_000));
        assert_eq!(
            zero.quantity,
            Ok(Quantity::Cpu(CpuSeconds {
                significand: 0,
                exponent: 0
            }))
        );
        assert_eq!(intervals[2].quantity, Err(Unresolved::Coverage));
    }

    #[test]
    fn keeps_disputes_and_competing_owners_in_physical_timestamp_order() {
        let original = binding(BindingKind::Home);
        let mut competitor = original.clone();
        competitor.owner_subject = "owner-b".into();
        let metric = SourceMetric::PvcUsed;
        let (first, _) = retained(&original, metric, 30_000, 15.0, 1);
        let (disputed, _) = retained(&original, metric, 60_000, 28.0, 1);
        let (other_owner, _) = retained(&competitor, metric, 60_000, 28.0, 1);
        let (last, _) = retained(&original, metric, 90_000, 32.0, 1);
        let normalize = |middle| {
            normalize_series(&[
                CustodySample::Undisputed(&last),
                CustodySample::Undisputed(&first),
                middle,
            ])
            .unwrap()
        };
        let disputed_intervals = normalize(CustodySample::Disputed(&disputed));
        assert_eq!(
            (disputed_intervals[0].from_ms, disputed_intervals[1].to_ms),
            (30_000, 90_000)
        );
        assert_eq!(
            disputed_intervals[0].evidence_sha256[1],
            disputed_intervals[1].evidence_sha256[0]
        );
        assert!(disputed_intervals
            .iter()
            .all(|pair| pair.quantity == Err(Unresolved::Disputed)));
        let conflicting = normalize(CustodySample::Undisputed(&other_owner));
        assert!(conflicting
            .iter()
            .all(|pair| pair.quantity == Err(Unresolved::Attribution)));
    }

    #[test]
    fn higher_binding_revision_controls_both_endpoints_independently_of_capture_order() {
        let original = binding(BindingKind::Compute);
        let metric = SourceMetric::WorkingSet;
        let (last, _) = retained(&original, metric, 40_000, 29.0, 8);
        let mut newer = original.clone();
        newer.revision += 1;
        newer.valid_until = None;
        newer.observed_at = 40;
        let (first, _) = retained(&newer, metric, 30_000, 17.0, 2);
        assert_eq!(pair(&last, &first), Err(Unresolved::Attribution));
        newer.observed_at += 1;
        let (extended, _) = retained(&newer, metric, 30_000, 17.0, 2);
        assert_eq!(
            pair(&last, &extended).unwrap().quantity,
            Ok(Quantity::ByteMilliseconds(170_000))
        );
        newer.id.push_str("-replacement");
        let (replacement, _) = retained(&newer, metric, 30_000, 17.0, 2);
        assert_eq!(pair(&last, &replacement), Err(Unresolved::Attribution));
    }

    #[test]
    fn equal_binding_revisions_must_agree_and_gauges_require_integer_bytes() {
        let initial = binding(BindingKind::Home);
        for metric in [SourceMetric::PvcUsed, SourceMetric::PvcCapacity] {
            let (first, _) = retained(&initial, metric, 20_000, 17.0, 1);
            let (next, _) = retained(&initial, metric, 21_003, 25.0, 1);
            let interval = pair(&first, &next).unwrap();
            assert_eq!(interval.metric, metric);
            assert_eq!(
                interval.quantity,
                Ok(Quantity::ByteMilliseconds(17 * 1_003))
            );
            let (fractional, _) = retained(&initial, metric, 21_003, 25.5, 1);
            assert_eq!(pair(&first, &fractional), Err(Unresolved::Precision));
            let mut changed = initial.clone();
            changed.valid_from += 1;
            let (conflicting, _) = retained(&changed, metric, 21_003, 25.0, 2);
            assert_eq!(pair(&first, &conflicting), Err(Unresolved::Attribution));
        }
    }
}
