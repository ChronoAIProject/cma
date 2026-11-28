use super::*;
use allocation::Expectation;

#[test]
fn reused_completion_matches_original_bytes_across_members_and_grids() {
    let mut completion = Completion::default();
    let range = Range { from: 17, to: 193 };
    let measured = BTreeMap::from([(
        60,
        Bucket {
            from: 60,
            to: 120,
            integral: -0.,
            display_integral: -0.,
            covered_seconds: 13.5,
            expected_seconds: 14.25,
            min: Some(-0.),
            max: Some(0.25),
            ..Default::default()
        },
    )]);
    let empty = BTreeMap::new();
    let on = Expectation {
        from: 0,
        to: 200,
        running: Some(true),
        home: Some(false),
    };
    let unknown = Expectation {
        running: None,
        home: None,
        ..on.clone()
    };
    let partial = Expectation {
        to: 110,
        ..on.clone()
    };
    let overlap = Expectation {
        from: 90,
        ..unknown.clone()
    };
    for (sparse, expectations, metric, range, step) in [
        (&measured, &[on.clone()][..], Metric::Cpu, range, 60),
        (&empty, &[on.clone()][..], Metric::Memory, range, 60),
        (&measured, &[on.clone()][..], Metric::Disk, range, 60),
        (&measured, &[unknown.clone()][..], Metric::Disk, range, 60),
        (&empty, &[unknown.clone()][..], Metric::Memory, range, 60),
        (
            &measured,
            &[partial.clone(), overlap][..],
            Metric::Cpu,
            range,
            60,
        ),
        (&empty, &[partial][..], Metric::Cpu, range, 60),
        (&empty, &[][..], Metric::Disk, range, 60),
        (&measured, &[on.clone()][..], Metric::Cpu, range, 30),
        (
            &measured,
            &[on.clone()][..],
            Metric::Cpu,
            Range { from: 17, to: 17 },
            60,
        ),
        (&measured, &[on][..], Metric::Cpu, range, 60),
    ] {
        let want = complete(sparse, expectations, metric, range, step);
        let got = completion.complete(sparse, expectations, metric, range, step);
        assert_eq!(
            serde_json::to_vec(got).unwrap(),
            serde_json::to_vec(&want).unwrap(),
            "metric={metric:?} range={range:?} step={step}"
        );
    }
}
