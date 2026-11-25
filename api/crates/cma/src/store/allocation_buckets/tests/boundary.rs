use super::*;

#[test]
fn partial_day_inputs_preserve_the_original_fold_and_gap_semantics() {
    let from = 100 * DAY;
    for mode in 0..4 {
        let mut s = state(1, from + 17);
        s.last_observed_at = from + 3 * HOUR;
        s.revision = 3;
        let mut events = vec![
            event(&s, 1, from + 17, from + HOUR),
            event(&s, 2, from + HOUR, from + 2 * HOUR),
            event(&s, 3, from + 2 * HOUR, from + 3 * HOUR),
        ];
        match mode {
            1 => events[1].kind = UsageEventKind::Baseline,
            2 => events[1].started_at -= 113,
            3 => {
                events[1].revision = 3;
                events[2].revision = 4;
                s.revision = 4;
            }
            _ => {}
        }
        let bucket = materialize::build_covered(
            &s,
            &events,
            Range {
                from,
                to: from + 2 * HOUR,
            },
            from + DAY,
        )
        .unwrap();
        let document = bucket.document().unwrap();
        let decoded = DayBucket::read_boundary(&document).unwrap();
        for range in [
            Range {
                from: from + 67,
                to: from + HOUR + 113,
            },
            Range {
                from: from + HOUR,
                to: from + 3 * HOUR + 97,
            },
        ] {
            let selected: Vec<_> = events
                .iter()
                .filter(|e| e.ended_at > range.from && e.started_at < range.to)
                .cloned()
                .collect();
            assert_projection(
                &decoded.project_boundary(&s, range).unwrap().unwrap(),
                &allocation::project(&s, &selected, range),
            );
        }
    }
}

#[test]
fn partial_day_inputs_reject_incomplete_coverage_foreign_scope_and_stale_tails() {
    let from = 100 * DAY;
    let mut s = state(1, from);
    s.last_observed_at = from + 2 * HOUR;
    s.revision = 2;
    let first = event(&s, 1, from, from + HOUR);
    let later = event(&s, 2, from + HOUR, from + 2 * HOUR);
    let range = Range {
        from: from + 1,
        to: from + 3 * HOUR,
    };
    let partial = materialize::build(
        &s,
        std::slice::from_ref(&first),
        Range {
            from,
            to: from + HOUR,
        },
    )
    .unwrap();
    assert!(partial.project_boundary(&s, range).unwrap().is_none());
    let complete = materialize::build_covered(
        &s,
        &[first.clone(), later.clone()],
        Range {
            from,
            to: from + HOUR,
        },
        from + DAY,
    )
    .unwrap();
    let mut next = s.clone();
    next.revision += 1;
    next.last_observed_at += HOUR;
    assert!(complete.project_boundary(&next, range).unwrap().is_none());
    let historical = Range {
        from: from + 17,
        to: from + HOUR - 31,
    };
    assert!(complete
        .project_boundary(&next, historical)
        .unwrap()
        .is_some());
    let mut foreign = later;
    foreign.owner = SubjectId("foreign".into());
    let invalid = materialize::build_covered(
        &s,
        &[first.clone(), foreign],
        Range {
            from,
            to: from + HOUR,
        },
        from + DAY,
    )
    .unwrap();
    assert!(invalid.project_boundary(&s, historical).unwrap().is_none());
    // A foreign interval outside the total selection must not taint that total.
    assert_projection(
        &invalid.total().projection,
        &allocation::project(
            &s,
            &[first],
            Range {
                from,
                to: from + HOUR,
            },
        ),
    );
}

#[test]
fn boundary_binary_digest_and_source_binding_refuse_tampering() {
    let from = 100 * DAY;
    let s = state(1, from);
    let bucket = materialize::build(
        &s,
        &[],
        Range {
            from,
            to: from + HOUR,
        },
    )
    .unwrap();
    let full = bucket.document().unwrap();
    let mut corrupt = full.clone();
    let mut bytes = corrupt
        .get_binary_generic("boundary_record")
        .unwrap()
        .clone();
    bytes[7] ^= 1;
    corrupt.insert(
        "boundary_record",
        mongodb::bson::Binary {
            subtype: mongodb::bson::spec::BinarySubtype::Generic,
            bytes,
        },
    );
    assert!(DayBucket::read_boundary(&corrupt).is_err());
    let other = materialize::build(
        &state(2, from),
        &[],
        Range {
            from,
            to: from + HOUR,
        },
    )
    .unwrap()
    .document()
    .unwrap();
    let mut swapped = full;
    swapped.insert(
        "boundary_record",
        other.get("boundary_record").unwrap().clone(),
    );
    swapped.insert(
        "boundary_sha256",
        other.get("boundary_sha256").unwrap().clone(),
    );
    assert!(DayBucket::read_boundary(&swapped).is_err());
}

#[test]
fn fully_queried_closed_incomplete_days_reuse_exact_unknown_values_across_heads() {
    let from = 100 * DAY;
    let mut s = state(1, from);
    s.tracking_started_at += HOUR;
    s.complete_lifecycle = false;
    s.last_observed_at = from + DAY;
    s.revision = 2;
    let mut baseline = event(&s, 1, from + HOUR, from + HOUR);
    baseline.kind = UsageEventKind::Baseline;
    let events = vec![baseline, event(&s, 2, from + HOUR, from + DAY)];
    let full = Range {
        from,
        to: from + DAY,
    };
    let bucket = materialize::build(&s, &events, full).unwrap();
    assert!(!bucket.total().projection.period.complete);
    assert_eq!(bucket.total().projection.period.unknown_seconds, HOUR);
    let mut next = s.clone();
    next.revision += 1;
    next.source_seq += 1;
    next.source_revision += 1;
    next.last_observed_at += HOUR;
    next.allocation.cpu_millis += 17;
    let mut appended = events.clone();
    appended.push(event(&next, 3, s.last_observed_at, next.last_observed_at));
    assert!(bucket.validate_source(&next).unwrap());
    let summary = DayBucket::read_summary(&bucket.document().unwrap()).unwrap();
    assert!(!summary.has_boundary());
    assert!(summary.validate_source(&next).unwrap());
    assert_projection(
        &summary.total().projection,
        &allocation::project(&next, &appended, full),
    );
    assert_projection(
        &bucket.total().projection,
        &allocation::project(&next, &appended, full),
    );
    let edge = Range {
        from: from + 17,
        to: from + 2 * HOUR + 11,
    };
    assert_projection(
        &bucket.project_boundary(&next, edge).unwrap().unwrap(),
        &allocation::project(&next, &appended, edge),
    );
    let partial = materialize::build(
        &s,
        &events,
        Range {
            from,
            to: from + 2 * HOUR,
        },
    )
    .unwrap();
    assert!(!partial.validate_source(&next).unwrap());
    next.tracking_started_at += 1;
    assert!(bucket.validate_source(&next).is_err());
}
