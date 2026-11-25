use super::*;

#[test]
fn expiry_without_original_process_evidence_cannot_claim_no_post() {
    for retained in [false, true] {
        let m = Measurement::new(1).unwrap();
        if retained {
            m.admitted("retained", 123, 1, None);
        }
        m.expired("retained", true);
        let row = m.snapshot().samples.remove(0);
        assert!(row.expired_before_start_ns.is_none());
        assert!(row.expired_after_post_ns.is_none());
        assert!(row.completed_ns.is_none());
        assert_eq!(row.unknown, Some("retained_or_uncertain_admission"));
    }
}

#[test]
fn retry_and_replay_preserve_original_admission_and_first_socket_write() {
    let m = Measurement::new(2).unwrap();
    m.offered("synthetic");
    let lower = m.now();
    m.admitted("synthetic", 123, 1, Some(lower));
    let first = m.snapshot();
    m.offered("synthetic");
    m.admitted("synthetic", 999, 1, None);
    let attempt = m.attempt("synthetic", true);
    let at = Instant::now();
    attempt.written(at);
    attempt.written(at);
    let retry = m.attempt("synthetic", false);
    retry.written(Instant::now());
    retry.accepted();
    let rows = m.snapshot().samples;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].candidate_admitted_at_seconds, Some(123));
    assert_eq!(
        rows[0].admission_confirmed_ns,
        first.samples[0].admission_confirmed_ns
    );
    assert_eq!(rows[0].first_post_ns, Some(m.elapsed(at)));
    assert_eq!(rows[0].post_attempts, 2);
    assert_eq!(
        rows[0].accepted_attempt_post_ns,
        retry.written.get().copied()
    );
    assert!(rows[0].unknown.is_none());
}

#[test]
fn bounded_window_preserves_unknown_restart_and_overflow_instead_of_evicting() {
    let m = Measurement::new(1).unwrap();
    m.offered("retained");
    m.admitted("retained", 123, 1, None);
    m.offered("cannot-fit");
    let s = m.snapshot();
    assert!(s.overflow);
    assert_eq!(s.samples.len(), 1);
    assert_eq!(
        s.samples[0].unknown,
        Some("retained_or_uncertain_admission")
    );
    assert!(s.samples[0].admission_confirmed_ns.is_none());
    assert!(Measurement::new(0).is_err());
    assert!(Measurement::new(MAX_SAMPLES + 1).is_err());
    let json = serde_json::to_string(&s).unwrap();
    assert!(!json.contains("retained\""));
    assert!(!json.contains("cannot-fit"));
}

#[test]
fn executor_start_and_cancel_are_neither_provider_start_nor_completion() {
    let m = Measurement::new(1).unwrap();
    m.offered("synthetic");
    m.admitted("synthetic", 123, 1, Some(m.now()));
    let guard = m.executor("synthetic");
    let s = m.snapshot();
    assert_eq!(s.active_executors, 1);
    assert!(s.samples[0].first_post_ns.is_none());
    assert!(s.samples[0].completed_ns.is_none());
    drop(guard);
    let s = m.snapshot();
    assert_eq!(s.active_executors, 0);
    assert_eq!(s.peak_executors, 1);
    assert_eq!(s.executor_attempts, 1);
    assert_eq!(s.samples[0].cancellations, 1);
    assert!(s.samples[0].first_post_ns.is_none());
}

#[test]
fn provider_start_lag_reports_only_proven_single_target_bounds() {
    let measurement = Measurement::new(3).unwrap();
    measurement.admitted("single", 123, 1, Some(measurement.now()));
    measurement.admitted("retained", 123, 1, None);
    measurement.admitted("multiple", 123, 2, Some(measurement.now()));
    let snapshot = measurement.snapshot();
    let single = &snapshot.samples[0];
    let commit = single.commit_lower_ns.unwrap();
    let confirmed = single.admission_confirmed_ns.unwrap();
    let written = confirmed + CLOCK_RESOLUTION_NS + 10;
    assert_eq!(
        start_lag(single, written),
        Some((10, written - commit + CLOCK_RESOLUTION_NS))
    );
    assert_eq!(start_lag(single, commit), Some((0, CLOCK_RESOLUTION_NS)));
    for row in &snapshot.samples[1..] {
        assert_eq!(start_lag(row, written), None);
    }
    let mut conflicting = single.clone();
    conflicting.admission_confirmed_ns = Some(commit.saturating_sub(1));
    conflicting.commit_lower_ns = Some(commit + 1);
    assert_eq!(start_lag(&conflicting, written), None);
}

#[test]
fn oldest_local_candidate_upper_depends_on_proven_admission_bounds() {
    let measurement = Measurement::new(3).unwrap();
    measurement.admitted("pending", 123, 1, Some(measurement.now()));
    measurement.admitted("finished", 123, 1, None);
    measurement.completed("finished");
    let pending = &measurement.snapshot().samples[0];
    let now = pending.admission_confirmed_ns.unwrap() + CLOCK_RESOLUTION_NS + 10;
    let (_, upper) = start_lag(pending, now).unwrap();
    assert_eq!(
        oldest_local_candidate_upper(&measurement.0.state.lock().unwrap(), now),
        Some(upper)
    );

    measurement.admitted("uncertain", 123, 1, None);
    assert_eq!(
        oldest_local_candidate_upper(&measurement.0.state.lock().unwrap(), now),
        None
    );
}
