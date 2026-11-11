use super::*;

fn baseline(format: SourceFormat) -> Metrics {
    Metrics {
        format,
        active_operations: 6,
        active_authority: 3,
        hot_progress_slots: 0,
        hot_replay_entries: 0,
        retained_groups: 0,
        reclaimed_operations: 5_000,
        reclaimed_authority: 5_000,
        preserved_source_sha256: [7; 32],
        active_bytes: 8 * 4_096,
        byte_ceiling: 256 * 4_096,
        page_geometry: (format == SourceFormat::V2).then_some((8, 256)),
    }
}

fn work() -> CycleWork {
    CycleWork {
        groups: 1,
        operations: 6,
        authority: 3,
    }
}

fn completed(before: &Metrics) -> Metrics {
    let mut after = before.clone();
    after.retained_groups += 1;
    after.reclaimed_operations += 6;
    after.reclaimed_authority += 3;
    after
}

#[test]
fn qualification_requires_observed_deltas_past_both_legacy_bounds() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        let mut proof = Qualification::new(baseline(format)).unwrap();
        proof.record(&work(), completed(&proof.previous)).unwrap();
        assert!(
            proof.require_legacy_bounds().is_err(),
            "old totals are not this campaign"
        );
        for _ in 1..1_365 {
            proof.record(&work(), completed(&proof.previous)).unwrap();
        }
        assert!(
            proof.require_legacy_bounds().is_err(),
            "4,095 authority is below the bound"
        );
        proof.record(&work(), completed(&proof.previous)).unwrap();
        proof.require_legacy_bounds().unwrap();
    }
}

#[test]
fn qualification_rejects_missing_rows_changed_limits_and_lost_live_state() {
    for case in 0..13 {
        let baseline = baseline(SourceFormat::V2);
        let mut after = completed(&baseline);
        match case {
            0 => after.reclaimed_operations -= 1,
            1 => after.reclaimed_authority -= 1,
            2 => after.reclaimed_operations = 0,
            3 => after.retained_groups = 0,
            4 => after.active_operations -= 1,
            5 => after.active_authority += 1,
            6 => after.preserved_source_sha256 = [9; 32],
            7 => after.hot_progress_slots = 2,
            8 => after.hot_replay_entries = 1,
            9 => after.byte_ceiling *= 2,
            10 => after.page_geometry = Some((8, 512)),
            11 => after.active_bytes = after.byte_ceiling + 1,
            _ => after.page_geometry = Some((257, 256)),
        }
        let mut proof = Qualification::new(baseline.clone()).unwrap();
        assert!(proof.record(&work(), after).is_err(), "case {case}");
        assert_eq!(
            proof.previous, baseline,
            "refused evidence must not advance proof"
        );
    }
}

#[test]
fn qualification_rejects_empty_invalid_or_overflowed_measurements() {
    for case in 0..5 {
        let mut metrics = baseline(SourceFormat::V2);
        match case {
            0 => metrics.preserved_source_sha256 = [0; 32],
            1 => metrics.byte_ceiling = 0,
            2 => metrics.page_geometry = None,
            3 => metrics.page_geometry = Some((0, 256)),
            _ => metrics.format = SourceFormat::V1,
        }
        assert!(Qualification::new(metrics).is_err(), "case {case}");
    }
    let mut metrics = baseline(SourceFormat::V1);
    metrics.reclaimed_operations = u64::MAX;
    let mut proof = Qualification::new(metrics.clone()).unwrap();
    assert!(proof.record(&work(), metrics).is_err());
    for groups in [0, 65] {
        let metrics = baseline(SourceFormat::V1);
        let mut proof = Qualification::new(metrics.clone()).unwrap();
        let work = CycleWork { groups, ..work() };
        assert!(proof.record(&work, completed(&metrics)).is_err());
    }
}
