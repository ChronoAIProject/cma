use super::*;
use std::collections::BTreeMap;
fn minute(from: u64, covered: f64, value: f64) -> Minute {
    let a = Aggregate {
        integral: covered * value,
        display_integral: covered * value,
        covered_seconds: covered,
        expected_seconds: 60.,
        min: Some(value),
        max: Some(value),
        ..Default::default()
    };
    Minute {
        from,
        to: from + 60,
        metrics: [a.clone(), a.clone(), a],
    }
}
#[test]
fn missing_time_does_not_reduce_mean_and_fleet_sums_sandbox_means() {
    let range = Range { from: 0, to: 300 };
    let a = series::buckets(
        &[minute(0, 30., 4.), minute(60, 60., 1.)],
        Metric::Cpu,
        range,
        300,
    );
    let b = series::buckets(&[minute(0, 10., 10.)], Metric::Cpu, range, 300);
    assert_eq!(a[0].mean, Some(2.));
    let mut total = a[0].clone();
    total.merge(&b[0]);
    let point = total.point();
    assert_eq!(point.mean, Some(12.));
    assert_eq!(point.covered_seconds, 100.);
    assert_eq!(point.expected_seconds, 180.);
    assert_eq!(point.excluded_count, 2);
    assert_eq!(point.reason.as_deref(), Some("partial_coverage"));
    assert_eq!((point.min, point.max), (Some(11.), Some(14.)));
}
#[test]
fn temporal_merge_keeps_weighting_and_peak_envelope() {
    let mut a = series::buckets(
        &[minute(0, 30., 4.)],
        Metric::Cpu,
        Range { from: 0, to: 60 },
        300,
    )
    .remove(0);
    let b = series::buckets(
        &[minute(60, 60., 1.)],
        Metric::Cpu,
        Range { from: 60, to: 120 },
        300,
    )
    .remove(0);
    a.merge_time(&b);
    assert_eq!((a.from, a.to), (0, 120));
    assert_eq!((a.mean, a.min, a.max), (Some(2.), Some(1.), Some(4.)));
}
#[test]
fn absent_sandbox_contributes_explicit_gaps_and_exclusions_without_fake_zero() {
    let range = Range { from: 5, to: 125 };
    let expectations = vec![allocation::Expectation {
        from: 5,
        to: 125,
        running: Some(true),
        home: Some(true),
    }];
    let points = series::complete(&BTreeMap::new(), &expectations, Metric::Memory, range, 60);
    assert_eq!(points.len(), 3);
    assert_eq!((points[0].from, points[2].to), (5, 125));
    assert_eq!(points.iter().map(|p| p.expected_seconds).sum::<f64>(), 120.);
    for p in points {
        let p = p.point();
        assert_eq!(p.mean, None);
        assert_eq!(p.covered_seconds, 0.);
        assert_eq!(p.excluded_count, 1);
        assert_eq!(p.expected_seconds, p.unknown_mapping_seconds);
        assert_eq!(p.reason.as_deref(), Some("no_measurement"));
    }
}
#[test]
fn unknown_allocation_does_not_become_zero_expected_activity() {
    let expectations = vec![allocation::Expectation {
        from: 0,
        to: 60,
        running: None,
        home: None,
    }];
    let p = series::complete(
        &BTreeMap::new(),
        &expectations,
        Metric::Cpu,
        Range { from: 0, to: 60 },
        60,
    )
    .remove(0)
    .point();
    assert_eq!(p.unknown_mapping_seconds, 60.);
    assert_eq!(p.excluded_count, 1);
    assert_eq!(p.reason.as_deref(), Some("no_measurement"));
}

#[test]
fn uniform_missing_buckets_match_the_general_completion() {
    let range = Range { from: 5, to: 125 };
    let sparse = BTreeMap::from([(
        60,
        series::Bucket {
            from: 60,
            to: 120,
            covered_seconds: 30.,
            display_integral: 90.,
            expected_seconds: 45.,
            min: Some(2.),
            max: Some(5.),
            ..Default::default()
        },
    )]);
    let out_of_range = allocation::Expectation {
        from: 180,
        to: 240,
        running: Some(true),
        home: None,
    };
    for (running, home) in [
        (Some(true), None),
        (Some(false), Some(true)),
        (None, Some(false)),
    ] {
        let covering = allocation::Expectation {
            from: 0,
            to: 150,
            running,
            home,
        };
        for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
            let uniform =
                series::complete(&sparse, std::slice::from_ref(&covering), metric, range, 60);
            let general = series::complete(
                &sparse,
                &[covering.clone(), out_of_range.clone()],
                metric,
                range,
                60,
            );
            assert_eq!(
                serde_json::to_vec(&uniform).unwrap(),
                serde_json::to_vec(&general).unwrap()
            );
        }
    }
}

#[test]
fn unaligned_seven_days_default_to_valid_epoch_buckets() {
    let input = ReportInput {
        from: Some(1),
        to: Some(604801),
        ..Default::default()
    };
    assert_eq!(input.clone().validate(604801).unwrap().step_seconds, 600);
    for step in [60, 300, 420, 1000] {
        assert!(ReportInput {
            step_seconds: Some(step),
            ..input.clone()
        }
        .validate(604801)
        .is_err());
    }
    assert!(ReportInput {
        step_seconds: Some(900),
        ..input
    }
    .validate(604801)
    .is_ok());
}
#[test]
fn paused_runtime_and_unmounted_home_keep_last_observation() {
    let a = Aggregate {
        integral: 100.,
        display_integral: 100.,
        covered_seconds: 10.,
        expected_seconds: 60.,
        observed_at: Some(100.),
        latest: Some(10.),
        ..Default::default()
    };
    let cpu = Measurement::project(&a, Metric::Cpu, 300, false, 0.);
    let disk = Measurement::project(&a, Metric::Disk, 300, false, 0.);
    assert_eq!(cpu.state, MetricState::NotRunning);
    assert_eq!(disk.state, MetricState::Unmounted);
    assert_eq!(disk.latest, None);
    assert_eq!(disk.last_observed, Some(10.));
    assert_eq!(disk.observed_at, Some(100.));
    assert!(disk.observed_hours.unwrap() > 0.);
}
