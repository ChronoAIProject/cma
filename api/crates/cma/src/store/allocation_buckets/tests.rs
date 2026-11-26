use super::*;
use layer2_core::sandbox_usage::{UsageEvent, UsageEventKind, UsageTotals};
use layer2_core::{SandboxId, SubjectId};

mod boundary;
mod live;
mod mongo;
mod recovery;

fn state(id: u64, created_at: u64) -> SandboxUsage {
    SandboxUsage {
        sandbox_id: SandboxId(format!("sbx_{id:032x}")),
        owner: SubjectId("owner-fixture".into()),
        name: "Synthetic".into(),
        status: "active".into(),
        allocation: Allocation {
            running: true,
            cpu_millis: 1137,
            memory_mib: 1027,
            disk_gib: 13,
        },
        created_at,
        tracking_started_at: created_at,
        last_observed_at: created_at,
        source_seq: 1,
        source_revision: 1,
        revision: 1,
        has_started: true,
        complete_lifecycle: true,
        totals: UsageTotals::default(),
    }
}

fn event(state: &SandboxUsage, revision: u64, from: u64, to: u64) -> UsageEvent {
    UsageEvent {
        sandbox_id: state.sandbox_id.clone(),
        owner: state.owner.clone(),
        revision,
        source_seq: revision,
        kind: UsageEventKind::Checkpoint,
        started_at: from,
        ended_at: to,
        allocation: state.allocation.clone(),
        next_allocation: state.allocation.clone(),
        status: state.status.clone(),
        quantity: state.allocation.usage(to - from).unwrap(),
    }
}

fn assert_projection(actual: &allocation::Projection, expected: &allocation::Projection) {
    assert_eq!(actual.period, expected.period);
    assert_eq!(actual.expectations, expected.expectations);
}

#[test]
fn daily_and_hourly_buckets_equal_original_raw_fold_for_1_7_30_days() {
    let first = 100 * DAY;
    let mut state = state(1, first + 17);
    let mut events = Vec::new();
    let mut at = state.created_at;
    // Irregular intervals and quantities expose floating point regrouping.
    while at < first + 30 * DAY {
        let next = (at + 617).min(first + 30 * DAY);
        let revision = events.len() as u64 + 1;
        let mut e = event(&state, revision, at, next);
        e.allocation.cpu_millis += (revision % 7) as u32;
        e.allocation.disk_gib += (revision % 3) as u32;
        events.push(e);
        at = next;
    }
    state.last_observed_at = at;
    state.revision = events.len() as u64;
    state.source_seq = state.revision;
    state.source_revision = state.revision;
    for days in [1, 7, 30] {
        let mut raw = layer2_core::sandbox_metrics::AllocatedPeriod {
            complete: true,
            ..Default::default()
        };
        let mut prepared = raw.clone();
        for day in 0..days {
            let range = Range {
                from: first + day * DAY,
                to: first + (day + 1) * DAY,
            };
            let matching: Vec<_> = events
                .iter()
                .filter(|e| e.ended_at > range.from && e.started_at < range.to)
                .cloned()
                .collect();
            let b = materialize::build(&state, &matching, range).unwrap();
            assert_projection(
                &b.total().projection,
                &allocation::project(&state, &matching, range),
            );
            for hour in b.hours() {
                assert_projection(
                    &hour.projection,
                    &allocation::project(&state, &matching, hour.range),
                );
            }
            assert_eq!(
                DayBucket::read(&b.document().unwrap()).unwrap().revision(),
                state.revision
            );
            raw.merge(&allocation::project(&state, &matching, range).period);
            prepared.merge(&b.total().projection.period);
        }
        assert_eq!(raw, prepared);
    }
}

#[test]
fn gaps_overlaps_baselines_and_projected_tails_preserve_raw_failure_semantics() {
    let from = 100 * DAY;
    for mode in 0..5 {
        let mut s = state(1, from);
        let mut events = vec![
            event(&s, 1, from, from + HOUR),
            event(&s, 2, from + HOUR, from + 2 * HOUR),
        ];
        s.last_observed_at = from + 2 * HOUR;
        s.revision = 2;
        match mode {
            0 => events[1].revision = 3,
            1 => events[1].started_at -= 30,
            2 => events[0].kind = UsageEventKind::Baseline,
            3 => events[0].owner = SubjectId("different".into()),
            _ => {
                s.tracking_started_at = from + 30;
                s.complete_lifecycle = false;
            }
        }
        s.revision = events.last().unwrap().revision;
        let range = Range {
            from,
            to: from + 3 * HOUR,
        };
        let b = materialize::build(&s, &events, range).unwrap();
        assert_projection(
            &b.total().projection,
            &allocation::project(&s, &events, range),
        );
        assert!(!b.total().projection.period.complete);
        let mut next = s.clone();
        next.revision += 1;
        assert!(!b.validate_source(&next).unwrap());
    }
}

#[test]
fn immutable_coverage_reuses_later_heads_but_rejects_same_revision_drift() {
    let from = 100 * DAY;
    let mut s = state(1, from);
    s.last_observed_at = from + DAY;
    let b = materialize::build(
        &s,
        &[event(&s, 1, from, from + DAY)],
        Range {
            from,
            to: from + DAY,
        },
    )
    .unwrap();
    let mut next = s.clone();
    next.revision += 1;
    next.last_observed_at += 60;
    assert!(b.validate_source(&next).unwrap());
    next.owner = SubjectId("other".into());
    assert!(b.validate_source(&next).is_err());
    next = s.clone();
    next.allocation.disk_gib += 1;
    assert!(b.validate_source(&next).is_err());
    next = s.clone();
    next.source_seq += 1;
    assert!(b.validate_source(&next).is_err());
    next = s.clone();
    next.revision += 1;
    next.last_observed_at -= 1;
    assert!(b.validate_source(&next).is_err());
    next = s.clone();
    next.revision -= 1;
    assert!(!b.validate_source(&next).unwrap());

    next = s.clone();
    next.complete_lifecycle = false;
    assert!(b.validate_source(&next).is_err());
    next.revision += 1;
    assert!(!b.validate_source(&next).unwrap());
    next.source_seq -= 1;
    assert!(b.validate_source(&next).is_err());
    next = s.clone();
    next.revision -= 1;
    next.complete_lifecycle = false;
    assert!(b.validate_source(&next).is_err());
    let incomplete = materialize::build(
        &SandboxUsage {
            complete_lifecycle: false,
            ..s.clone()
        },
        &[event(&s, 1, from, from + DAY)],
        Range {
            from,
            to: from + DAY,
        },
    )
    .unwrap();
    next = s;
    next.revision += 1;
    assert!(incomplete.validate_source(&next).is_err());
}

#[test]
fn schema_digest_and_order_checks_refuse_corrupt_evidence() {
    let from = 100 * DAY;
    let s = state(1, from);
    let b = materialize::build(
        &s,
        &[],
        Range {
            from,
            to: from + HOUR,
        },
    )
    .unwrap();
    for field in [
        "_id",
        "sandbox_id",
        "owner",
        "day",
        "revision",
        "cutoff",
        "content_sha256",
    ] {
        let mut document = b.document().unwrap();
        document.insert(field, "bad");
        assert!(DayBucket::read(&document).is_err(), "{field}");
    }
    let mut malformed = mongodb::bson::serialize_to_document(&b).unwrap();
    malformed
        .get_document_mut("total")
        .unwrap()
        .get_document_mut("projection")
        .unwrap()
        .get_document_mut("period")
        .unwrap()
        .insert("cpu_core_hours", f64::NAN);
    let malformed: DayBucket = mongodb::bson::deserialize_from_document(malformed).unwrap();
    assert!(malformed.document().is_err());
    assert!(materialize::build(
        &s,
        &[],
        Range {
            from: from + 1,
            to: from + HOUR
        }
    )
    .is_err());
    assert!(materialize::build(
        &s,
        &[event(&s, 2, from, from + 1)],
        Range {
            from,
            to: from + HOUR
        }
    )
    .is_err());
}

#[test]
fn compact_daily_summary_is_exact_and_independently_validated() {
    let from = 100 * DAY;
    let s = state(1, from);
    let bucket = materialize::build(
        &s,
        &[],
        Range {
            from,
            to: from + DAY,
        },
    )
    .unwrap();
    let full = bucket.document().unwrap();
    let mut projected = full.clone();
    projected.remove("record");
    projected.remove("content_sha256");
    let summary = DayBucket::read_summary(&projected).unwrap();
    assert!(summary.hours().is_empty());
    assert!(summary.validate_source(&s).unwrap());
    assert_projection(&summary.total().projection, &bucket.total().projection);
    assert!(DayBucket::read(&projected).is_err());
    for field in [
        "summary_record",
        "summary_sha256",
        "document_schema_version",
        "owner",
        "day",
    ] {
        let mut corrupt = projected.clone();
        corrupt.remove(field);
        assert!(DayBucket::read_summary(&corrupt).is_err(), "{field}");
    }
    let full_bytes = mongodb::bson::serialize_to_vec(&full).unwrap().len();
    let summary_bytes = mongodb::bson::serialize_to_vec(&projected).unwrap().len();
    assert!(summary_bytes < full_bytes / 4);
    eprintln!("allocation bucket full_bytes={full_bytes} summary_bytes={summary_bytes}");
}
