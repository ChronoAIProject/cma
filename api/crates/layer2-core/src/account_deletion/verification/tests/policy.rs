use super::*;

fn approval() -> RetentionApproval {
    RetentionApproval {
        policy_exception_digest: digest("approved retention exception"),
        expires_at: "2026-11-23T00:00:00Z".into(),
        deidentification_digest: digest("validated deidentification"),
    }
}

#[test]
fn retained_observation_requires_the_exact_approved_exception_and_scan() {
    let (mut plan, mut observations) = fixture();
    let approval = approval();
    let mut scan = scan();
    scan.pages[0].remaining = 4;
    observations.observations[0].observation = ResidueObservation::ApprovedRetained {
        scan: scan.clone(),
        approval: approval.clone(),
    };
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::PolicyMismatch
    );
    plan.requirements[0].disposition = RequiredDisposition::ApprovedRetained {
        approval: approval.clone(),
    };
    plan.validate(&observations, NOW).unwrap();
    let mut changed = approval.clone();
    changed.policy_exception_digest = digest("different exception");
    observations.observations[0].observation = ResidueObservation::ApprovedRetained {
        scan: scan.clone(),
        approval: changed,
    };
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::PolicyMismatch
    );
    let mut changed = approval.clone();
    changed.deidentification_digest = digest("different deidentification");
    observations.observations[0].observation = ResidueObservation::ApprovedRetained {
        scan: scan.clone(),
        approval: changed,
    };
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::PolicyMismatch
    );
    let mut changed = approval;
    changed.expires_at = "2026-11-24T00:00:00Z".into();
    observations.observations[0].observation = ResidueObservation::ApprovedRetained {
        scan,
        approval: changed,
    };
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::PolicyMismatch
    );
}

#[test]
fn expired_retention_or_partial_retained_scan_is_incomplete() {
    let (mut plan, mut observations) = fixture();
    let mut approval = approval();
    approval.expires_at = NOW.into();
    plan.requirements[0].disposition = RequiredDisposition::ApprovedRetained {
        approval: approval.clone(),
    };
    observations.observations[0].observation = ResidueObservation::ApprovedRetained {
        scan: scan(),
        approval: approval.clone(),
    };
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::RetentionExpired
    );
    approval.expires_at = "2026-11-23T00:00:00Z".into();
    plan.requirements[0].disposition = RequiredDisposition::ApprovedRetained {
        approval: approval.clone(),
    };
    let mut partial = scan();
    partial.pages[0].next_cursor = Some(digest("unread page"));
    observations.observations[0].observation = ResidueObservation::ApprovedRetained {
        scan: partial,
        approval,
    };
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::IncompleteScan
    );
}

#[test]
fn not_applicable_requires_positive_policy_bound_authority_and_fresh_observation() {
    let (mut plan, mut observations) = fixture();
    let evidence = digest("qualified absence authority");
    observations.observations[0].observation = ResidueObservation::ProvenNotApplicable {
        observed_at: NOW.into(),
        authority_evidence: evidence.clone(),
    };
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::PolicyMismatch
    );
    plan.requirements[0].disposition = RequiredDisposition::ProvenNotApplicable {
        authority_evidence: evidence.clone(),
    };
    plan.validate(&observations, NOW).unwrap();
    observations.observations[0].observation = ResidueObservation::ProvenNotApplicable {
        observed_at: NOW.into(),
        authority_evidence: digest("unrelated absence"),
    };
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::PolicyMismatch
    );
    observations.observations[0].observation = ResidueObservation::ProvenNotApplicable {
        observed_at: "2026-11-22T00:00:00Z".into(),
        authority_evidence: evidence,
    };
    assert_eq!(
        plan.validate(&observations, NOW).unwrap_err(),
        CoverageError::InvalidTime
    );
}
