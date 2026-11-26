use super::*;

fn expectation(from: u64, to: u64, running: Option<bool>, home: Option<bool>) -> Expectation {
    Expectation {
        from,
        to,
        running,
        home,
    }
}

#[test]
fn metric_intervals_preserve_complete_buckets_for_all_state_transitions() {
    let values = [None, Some(false), Some(true)];
    for running in values {
        for home in values {
            for next_running in values {
                for next_home in values {
                    for next_from in [50, 60, 70] {
                        let input = vec![
                            expectation(5, 60, running, home),
                            expectation(next_from, 125, next_running, next_home),
                        ];
                        for metric in [Metric::Cpu, Metric::Memory, Metric::Disk] {
                            let compact = metric_expectations(&input, metric);
                            for (from, to, step) in [(0, 150, 60), (13, 119, 17), (5, 125, 300)] {
                                let range = layer2_core::sandbox_metrics::Range { from, to };
                                for measured in [false, true] {
                                    let mut sparse = BTreeMap::new();
                                    if measured {
                                        let mut bucket = series::buckets(&[], metric, range, step)
                                            .into_iter()
                                            .next()
                                            .unwrap();
                                        bucket.covered_seconds = 7.;
                                        bucket.expected_seconds = 11.;
                                        bucket.display_integral = 19.25;
                                        bucket.integral = 21.5;
                                        bucket.min = Some(1.25);
                                        bucket.max = Some(4.75);
                                        sparse.insert(bucket.from, bucket);
                                    }
                                    let before =
                                        series::complete(&sparse, &input, metric, range, step);
                                    let after =
                                        series::complete(&sparse, &compact, metric, range, step);
                                    assert_eq!(
                                        serde_json::to_value(before).unwrap(),
                                        serde_json::to_value(after).unwrap(),
                                        "{metric:?} {input:?} {range:?} step={step} measured={measured}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn metric_compaction_retains_gaps_overlaps_unknowns_and_empty_intervals() {
    let input = vec![
        expectation(0, 60, Some(true), Some(true)),
        expectation(60, 120, None, Some(true)),
        expectation(120, 180, Some(false), Some(true)),
        expectation(190, 200, Some(false), Some(true)),
        expectation(195, 210, Some(false), Some(true)),
        expectation(210, 210, Some(false), Some(true)),
    ];
    assert_eq!(metric_expectations(&input, Metric::Cpu), input);
    let disk = metric_expectations(&input, Metric::Disk);
    assert_eq!(disk.len(), 4);
    assert_eq!(disk[0], expectation(0, 180, None, Some(true)));
    assert_eq!(&disk[1..], &input[3..]);
    assert!(metric_expectations(&[], Metric::Memory).is_empty());
}

#[test]
fn disk_compaction_reduces_payload_with_legacy_readable_chart_fields() {
    let input: Vec<_> = (0..1440)
        .map(|n| expectation(n * 1800, (n + 1) * 1800, Some(n % 5 != 0), Some(true)))
        .collect();
    let compact = metric_expectations(&input, Metric::Disk);
    assert_eq!(compact, vec![expectation(0, 30 * 86400, None, Some(true))]);
    let before = mongodb::bson::serialize_to_vec(&super::super::chart_encoding::ChartRecord {
        buckets: &[],
        expectations: &input,
    })
    .unwrap();
    let after = mongodb::bson::serialize_to_vec(&super::super::chart_encoding::ChartRecord {
        buckets: &[],
        expectations: &compact,
    })
    .unwrap();
    assert!(after.len() * 100 < before.len());
    let decoded: ChartChunk = mongodb::bson::deserialize_from_slice(&after).unwrap();
    assert!(decoded.buckets.is_empty());
    assert_eq!(decoded.expectations, compact);
    let range = layer2_core::sandbox_metrics::Range {
        from: 67,
        to: 30 * 86400 - 113,
    };
    let old = series::complete(&BTreeMap::new(), &input, Metric::Disk, range, 3600);
    let new = series::complete(
        &BTreeMap::new(),
        &decoded.expectations,
        Metric::Disk,
        range,
        3600,
    );
    assert_eq!(
        serde_json::to_value(old).unwrap(),
        serde_json::to_value(new).unwrap()
    );
}
