use super::*;

fn check(scan: CompleteScan) -> Result<(), CoverageError> {
    let (plan, mut observations) = fixture();
    observations.observations[0].observation = ResidueObservation::ObservedZero { scan };
    plan.validate(&observations, NOW).map(|_| ())
}

fn paged() -> CompleteScan {
    let mut scan = scan();
    let cursor = digest("page two cursor");
    scan.pages[0].next_cursor = Some(cursor.clone());
    scan.pages.push(ScanPage {
        cursor: Some(cursor),
        next_cursor: None,
        remaining: 0,
        evidence_digest: digest("second complete page"),
    });
    scan
}

#[test]
fn complete_empty_pagination_is_accepted_but_nonzero_later_page_is_not() {
    let mut scan = paged();
    check(scan.clone()).unwrap();
    scan.pages[1].remaining = 1;
    assert_eq!(check(scan), Err(CoverageError::ResidueFound));
}

#[test]
fn incomplete_missing_repeated_and_spliced_pages_are_rejected() {
    let base = paged();
    let mut empty = base.clone();
    empty.pages.clear();
    let mut partial = base.clone();
    partial.pages.pop();
    let mut skipped = base.clone();
    skipped.pages.remove(0);
    let mut spliced = base.clone();
    spliced.pages[1].cursor = Some(digest("unrelated cursor"));
    let mut early_end = base.clone();
    early_end.pages[0].next_cursor = None;
    let mut repeated = base.clone();
    repeated.pages[1].next_cursor = repeated.pages[1].cursor.clone();
    repeated.pages.push(base.pages[1].clone());
    let mut restarted = base;
    restarted.pages[1].cursor = None;
    for scan in [
        empty, partial, skipped, spliced, early_end, repeated, restarted,
    ] {
        assert_eq!(check(scan), Err(CoverageError::IncompleteScan));
    }
}

#[test]
fn scan_limits_and_count_overflow_never_truncate_residue() {
    let mut scan = paged();
    scan.pages[0].remaining = u64::MAX;
    scan.pages[1].remaining = 1;
    assert_eq!(check(scan), Err(CoverageError::CapacityExceeded));
    let mut scan = super::scan();
    scan.pages = vec![scan.pages[0].clone(); MAX_ERASURE_SCAN_PAGES + 1];
    assert_eq!(check(scan), Err(CoverageError::CapacityExceeded));
}

#[test]
fn scan_requires_ordered_fresh_times_including_subsecond_boundaries() {
    let base = scan();
    for start in [
        "bad",
        "2026-11-22T00:00:59.999999999Z",
        "2026-11-22T01:00:00.000000001Z",
    ] {
        let mut scan = base.clone();
        scan.started_at = start.into();
        assert_eq!(check(scan), Err(CoverageError::InvalidTime));
    }
    for end in [
        "bad",
        "2026-11-22T00:01:59.999999999Z",
        "2026-11-22T01:00:00.000000001Z",
    ] {
        let mut scan = base.clone();
        scan.finished_at = end.into();
        assert_eq!(check(scan), Err(CoverageError::InvalidTime));
    }
    let (mut plan, mut observations) = fixture();
    plan.binding.checkpoint.writers_closed_at = "2026-11-22T00:02:00.000000001Z".into();
    observations.binding = plan.binding.clone();
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::InvalidTime
    );
}

#[test]
fn all_incomplete_causes_and_explicit_residuals_block_a_zero_projection() {
    let (plan, mut observations) = fixture();
    for reason in [
        IncompleteReason::PermissionDenied,
        IncompleteReason::Unavailable,
        IncompleteReason::Unsupported,
        IncompleteReason::UnknownFormat,
        IncompleteReason::UnqualifiedAbsence,
        IncompleteReason::PartialListing,
        IncompleteReason::MissingPolicy,
    ] {
        observations.observations[0].observation = ResidueObservation::Incomplete { reason };
        assert_eq!(
            plan.validate(&observations, NOW).unwrap_err(),
            CoverageError::Incomplete
        );
    }
    observations.observations[0].observation = ResidueObservation::Residual { scan: scan() };
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::ResidueFound
    );
}
