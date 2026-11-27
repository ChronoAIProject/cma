use super::*;
use layer3_adapter::sandbox_metrics::{PhysicalSeries, Sample, SourceMetric};
use std::collections::BTreeMap;

pub(super) fn binding(kind: BindingKind) -> Binding {
    Binding {
        id: format!("{kind:?}"),
        schema_version: 1,
        revision: 1,
        deployment: "cma".into(),
        cluster: "prod".into(),
        namespace: "runtime".into(),
        native_sandbox_uid: "native-uid".into(),
        claim_uid: "claim-uid".into(),
        sandbox_id: "sbx".into(),
        owner_subject: "owner-a".into(),
        pod_name: Some("cold-claim".into()),
        pod_uid: Some("pod-uid".into()),
        runtime_container_id: Some("container-1".into()),
        restart_count: Some(0),
        runtime_generation: Some(1),
        pvc_name: "home-cold-claim".into(),
        pvc_uid: "volume-uid".into(),
        kind,
        cpu_limit_cores: Some(2.0),
        memory_limit_bytes: Some(1024.0),
        valid_from: 0,
        valid_until: Some(600),
        observed_at: 600,
        proof_version: 1,
        proof_status: ProofStatus::Verified,
    }
}
fn series(metric: SourceMetric, values: &[(f64, f64)]) -> PhysicalSeries {
    PhysicalSeries {
        metric,
        name: if metric.is_pvc() {
            "home-cold-claim"
        } else {
            "cold-claim"
        }
        .into(),
        labels: BTreeMap::from([
            ("cluster".into(), "prod".into()),
            ("namespace".into(), "runtime".into()),
        ]),
        samples: values
            .iter()
            .map(|(at, value)| Sample {
                at: *at,
                value: Some(*value),
            })
            .collect(),
    }
}
#[test]
fn resets_long_gaps_and_assignment_boundaries_never_invent_cpu() {
    let s = series(
        SourceMetric::CpuCounter,
        &[
            (0., 0.),
            (30., 60.),
            (60., 3.),
            (90., 33.),
            (300., 600.),
            (330., 630.),
        ],
    );
    let mut b = binding(BindingKind::Compute);
    let t = aggregate::totals(
        &aggregate::aggregate(
            &[b.clone()],
            std::slice::from_ref(&s),
            Range { from: 0, to: 360 },
        )
        .unwrap(),
    );
    assert_eq!(t[0].integral, 120.);
    assert_eq!(t[0].covered_seconds, 90.);
    assert_eq!(t[0].expected_seconds, 360.);
    b.valid_from = 45;
    let t =
        aggregate::totals(&aggregate::aggregate(&[b], &[s], Range { from: 75, to: 315 }).unwrap());
    assert_eq!(t[0].integral, 30.);
    assert_eq!(t[0].covered_seconds, 30.);
}
#[test]
fn irregular_gauges_hold_at_most_ninety_seconds_and_clip_edges() {
    let s = series(
        SourceMetric::WorkingSet,
        &[(0., 100.), (20., 200.), (200., 300.)],
    );
    let t = aggregate::totals(
        &aggregate::aggregate(
            &[binding(BindingKind::Compute)],
            &[s],
            Range { from: 10, to: 240 },
        )
        .unwrap(),
    );
    assert_eq!(t[1].integral, 100. * 10. + 200. * 90. + 300. * 40.);
    assert_eq!(t[1].covered_seconds, 140.);
    assert_eq!(t[1].expected_seconds, 230.);
}
#[test]
fn disk_capacity_changes_and_expires_independently_of_usage() {
    let values = [
        series(SourceMetric::PvcUsed, &[(30., 100.), (90., 200.)]),
        series(SourceMetric::PvcCapacity, &[(0., 1000.), (60., 2000.)]),
    ];
    let totals = aggregate::totals(
        &aggregate::aggregate(
            &[binding(BindingKind::Home)],
            &values,
            Range { from: 30, to: 180 },
        )
        .unwrap(),
    );
    let disk = &totals[2];
    assert_eq!(disk.covered_seconds, 150.);
    assert_eq!(disk.integral, 100. * 60. + 200. * 90.);
    assert_eq!(disk.capacity_covered_seconds, 120.);
    assert_eq!(disk.capacity_integral, 1000. * 30. + 2000. * 90.);
    assert_eq!(disk.capacity_usage_integral, 100. * 60. + 200. * 60.);
}

#[test]
fn stale_samples_end_counter_gauge_and_disk_capacity_coverage() {
    let mut cpu = series(
        SourceMetric::CpuCounter,
        &[(0., 0.), (20., 20.), (30., 0.), (40., 90.), (60., 130.)],
    );
    cpu.samples[2].value = None;
    let mut memory = series(
        SourceMetric::WorkingSet,
        &[(0., 100.), (30., 0.), (60., 200.)],
    );
    memory.samples[1].value = None;
    let disk = series(SourceMetric::PvcUsed, &[(0., 100.)]);
    let mut capacity = series(
        SourceMetric::PvcCapacity,
        &[(0., 1000.), (30., 0.), (60., 2000.)],
    );
    capacity.samples[1].value = None;

    let totals = aggregate::totals(
        &aggregate::aggregate(
            &[binding(BindingKind::Compute), binding(BindingKind::Home)],
            &[cpu, memory, disk, capacity],
            Range { from: 0, to: 90 },
        )
        .unwrap(),
    );
    assert_eq!(totals[0].covered_seconds, 40.);
    assert_eq!(totals[0].integral, 60.);
    assert_eq!(totals[0].display_integral, 60.);
    assert_eq!(totals[1].covered_seconds, 60.);
    assert_eq!(totals[1].integral, 9_000.);
    assert_eq!(totals[2].covered_seconds, 90.);
    assert_eq!(totals[2].capacity_covered_seconds, 60.);
    assert_eq!(totals[2].capacity_integral, 90_000.);
}
#[test]
fn shared_pvc_is_counted_once_and_conflicting_attachments_are_refused() {
    let b = binding(BindingKind::Home);
    let s = series(SourceMetric::PvcUsed, &[(0., 100.)]);
    let t = aggregate::totals(
        &aggregate::aggregate(
            &[b.clone(), b.clone()],
            std::slice::from_ref(&s),
            Range { from: 0, to: 60 },
        )
        .unwrap(),
    );
    assert_eq!(t[2].integral, 6000.);
    let mut other = b.clone();
    other.id = "reattached".into();
    other.owner_subject = "owner-b".into();
    assert!(aggregate::aggregate(&[b, other], &[s], Range { from: 0, to: 60 }).is_err());
}
#[test]
fn unknown_denominators_and_over_capacity_remain_visible() {
    let mut a = Aggregate {
        integral: 300.,
        capacity_usage_integral: 300.,
        covered_seconds: 60.,
        capacity_integral: 120.,
        capacity_covered_seconds: 60.,
        ..Default::default()
    };
    assert_eq!(a.percent(), Some(250.));
    a.capacity_integral = 0.;
    assert_eq!(a.percent(), None);
    a.capacity_integral = 120.;
    a.capacity_covered_seconds = 30.;
    a.capacity_usage_integral = 150.;
    assert_eq!(a.percent(), Some(125.));
}
#[test]
fn default_ranges_and_steps_are_bounded_and_typed() {
    let now = 2_000_000_000;
    let q = ReportInput::default().validate(now).unwrap();
    assert_eq!(q.step_seconds, 60);
    assert_eq!(q.to % 60, 0);
    assert!(ReportInput {
        from: Some(now - 604_800),
        to: Some(now),
        step_seconds: Some(60),
        ..Default::default()
    }
    .validate(now)
    .is_err());
    assert!(serde_json::from_str::<ReportInput>(r#"{"query":"up"}"#).is_err());
    assert!(
        serde_json::from_str::<ReportInput>(r#"{"filters":{"statuses":["invented"]}}"#).is_err()
    );
    assert!(ReportInput {
        to: Some(now + 1),
        ..Default::default()
    }
    .validate(now)
    .is_err());
}
#[test]
fn cursors_bind_every_scope_dimension_and_reject_tampering() {
    use cursor::*;
    let signer = CursorSigner::new(&[7; 32]);
    let scope = Scope {
        principal: "a".into(),
        admin: false,
    };
    let c = Cursor {
        principal: "a".into(),
        admin: false,
        filter_hash: "q".into(),
        report_id: "r".into(),
        sort: Sort::CpuMeanCores,
        order: Order::Desc,
        last_value: 1.0.into(),
        last_null: false,
        sandbox_id: "s".into(),
    };
    let token = signer.encode(&c).unwrap();
    assert_eq!(
        signer
            .decode(&token, &scope, "r", "q", c.sort, c.order)
            .unwrap(),
        c
    );
    for (scope, r, q, s, o) in [
        (
            Scope {
                principal: "b".into(),
                admin: false,
            },
            "r",
            "q",
            c.sort,
            c.order,
        ),
        (scope.clone(), "other", "q", c.sort, c.order),
        (scope.clone(), "r", "changed", c.sort, c.order),
        (scope.clone(), "r", "q", Sort::Name, c.order),
        (scope.clone(), "r", "q", c.sort, Order::Asc),
    ] {
        assert!(signer.decode(&token, &scope, r, q, s, o).is_err());
    }
    assert!(signer
        .decode(&(token + "x"), &scope, "r", "q", c.sort, c.order)
        .is_err());
}

#[test]
fn cursors_preserve_exact_fractional_subtotals_at_page_boundaries() {
    use cursor::*;
    let signer = CursorSigner::new(&[7; 32]);
    let scope = Scope {
        principal: "owner".into(),
        admin: false,
    };
    for subtotal in [
        0.0_f64,
        8.165,
        8.1654,
        0.46694444444444444,
        2.334722222222222,
    ] {
        for order in [Order::Asc, Order::Desc] {
            let cursor = Cursor {
                principal: scope.principal.clone(),
                admin: false,
                filter_hash: "query".into(),
                report_id: "report".into(),
                sort: Sort::AllocatedCpuCoreHours,
                order,
                last_value: subtotal.into(),
                last_null: false,
                sandbox_id: "boundary".into(),
            };
            let decoded = signer
                .decode(
                    &signer.encode(&cursor).unwrap(),
                    &scope,
                    "report",
                    "query",
                    cursor.sort,
                    order,
                )
                .unwrap();
            assert_eq!(
                decoded.last_value.as_f64().unwrap().to_bits(),
                subtotal.to_bits()
            );
        }
    }
}

#[test]
fn new_incarnation_never_reuses_an_old_baseline() {
    let mut first = binding(BindingKind::Compute);
    let mut second = first.clone();
    first.valid_until = Some(60);
    second.id = "next-incarnation".into();
    second.valid_from = 75;
    second.runtime_container_id = Some("container-2".into());
    second.restart_count = Some(1);
    let data = series(
        SourceMetric::CpuCounter,
        &[(0., 0.), (30., 30.), (60., 60.), (90., 90.), (120., 120.)],
    );
    let totals = aggregate::totals(
        &aggregate::aggregate(&[first, second], &[data], Range { from: 0, to: 120 }).unwrap(),
    );
    assert_eq!(totals[0].integral, 60.);
    assert_eq!(totals[0].covered_seconds, 60.);
    assert_eq!(totals[0].expected_seconds, 105.);
}
#[test]
fn cpu_display_uses_two_minutes_while_integral_uses_exact_deltas() {
    let data = series(
        SourceMetric::CpuCounter,
        &[(0., 0.), (30., 30.), (60., 90.), (90., 90.), (120., 120.)],
    );
    let totals = aggregate::totals(
        &aggregate::aggregate(
            &[binding(BindingKind::Compute)],
            &[data],
            Range { from: 95, to: 110 },
        )
        .unwrap(),
    );
    assert_eq!(totals[0].integral, 15.);
    assert_eq!(totals[0].covered_seconds, 15.);
    assert_eq!(totals[0].latest, Some(1.));
    assert_eq!(totals[0].observed_at, Some(120.));
}
