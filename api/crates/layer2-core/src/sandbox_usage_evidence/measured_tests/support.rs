use super::*;
use crate::sandbox_metrics::Metric;

#[test]
fn sampled_support_tracks_resets_and_submillisecond_intervals_before_rounding() {
    let reset = freeze(vec![series(
        SourceMetric::CpuCounter,
        &[(0.0, 10.0), (10.0, 20.0), (20.0, 5.0), (30.0, 15.0)],
    )]);
    let support = reset.observed_support().unwrap();
    assert_eq!(support.len(), 1);
    assert_eq!(support[0].metric, Metric::Cpu);
    assert_eq!(support[0].intervals, vec![(0.0, 10.0), (20.0, 30.0)]);
    assert_eq!(reset.fact().metrics[0].covered_binding_ms, "20000");

    let tiny = freeze(vec![series(
        SourceMetric::CpuCounter,
        &[(0.0001, 10.0), (0.0004, 10.0)],
    )]);
    assert_eq!(tiny.fact().metrics[0].covered_binding_ms, "0");
    assert_eq!(
        tiny.observed_support().unwrap()[0].intervals,
        vec![(0.0001, 0.0004)]
    );
}

#[test]
fn sampled_support_obeys_counter_gaps_and_both_gauge_hold_expiries() {
    let mut expected = expected();
    expected.bucket_start = 120;
    expected.compute = vec![Range { from: 120, to: 180 }];
    expected.home = expected.compute.clone();
    let bindings = vec![binding(BindingKind::Compute), binding(BindingKind::Home)];
    let values = vec![
        series(SourceMetric::CpuCounter, &[(0.0, 10.0), (150.0, 20.0)]),
        series(SourceMetric::WorkingSet, &[(0.0, 10.0), (150.0, 20.0)]),
        series(SourceMetric::PvcUsed, &[(0.0, 10.0), (150.0, 20.0)]),
    ];
    let frozen = freeze_measured(expected.clone(), bindings.clone(), values.clone(), true).unwrap();
    let support = frozen.observed_support().unwrap();
    assert_eq!(support.len(), 2);
    for metric in [Metric::Memory, Metric::Disk] {
        let observed = support.iter().find(|s| s.metric == metric).unwrap();
        assert_eq!(observed.intervals, vec![(150.0, 180.0)]);
        assert_eq!(
            frozen.fact().metrics[metric.index()].covered_binding_ms,
            "30000"
        );
    }
    assert_eq!(frozen.fact().metrics[0].covered_binding_ms, "0");
    let unavailable = freeze_measured(expected, bindings, values, false).unwrap();
    assert!(unavailable.observed_support().unwrap().is_empty());
}
