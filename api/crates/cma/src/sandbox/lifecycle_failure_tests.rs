use super::*;

#[test]
fn ownership_projection_retains_original_class_and_compatibility() {
    let attempt = LifecycleAttempt::new(Phase::Execution, Some(&AgentId("private-agent".into())));
    for (error, cause) in [
        (OwnershipError::Stale, CauseClass::OwnershipStale),
        (
            OwnershipError::Unavailable("private-detail".into()),
            CauseClass::OwnershipUnavailable,
        ),
        (
            OwnershipError::Invalid("private-detail".into()),
            CauseClass::OwnershipInvalid,
        ),
        (
            OwnershipError::IncompatibleProtocol,
            CauseClass::IncompatibleProtocol,
        ),
    ] {
        let failure = attempt.ownership(Origin::OwnershipAcquire, error);
        assert_eq!(failure.observation.cause_class, cause);
        assert!(!format!("{failure:?}").contains("private-"));
        assert_eq!(failure.into_compatibility(), SandboxStoreError::Conflict);
    }
}

#[test]
fn only_registered_exact_details_at_their_boundaries_are_retained() {
    let attempt = LifecycleAttempt::new(Phase::Discovery, None);
    let discovery = attempt.store(
        Origin::Discovery,
        SandboxStoreError::Unavailable(DISCOVERY_DEADLINE.into()),
    );
    assert_eq!(
        discovery.observation.detail_code,
        DetailCode::DiscoveryDeadline
    );
    assert!(discovery.observation.detail_retained && !discovery.observation.detail_suppressed);
    for (origin, text, code) in [
        (
            Origin::OwnershipAcquire,
            "ownership operation exceeded its deadline",
            DetailCode::OwnershipDeadline,
        ),
        (
            Origin::Handoff,
            "ownership operation exceeded its deadline",
            DetailCode::OwnershipDeadline,
        ),
        (
            Origin::Handoff,
            "receiving runtime revocation failed",
            DetailCode::ReceiverRevocationUnconfirmed,
        ),
        (
            Origin::Claim,
            "receiving runtime revocation failed",
            DetailCode::Suppressed,
        ),
        (
            Origin::OwnershipAcquire,
            "ownership operation exceeded its deadline: private",
            DetailCode::Suppressed,
        ),
    ] {
        let value = attempt.ownership(origin, OwnershipError::Unavailable(text.into()));
        assert_eq!(value.observation.detail_code, code);
    }
    let wrong_origin = attempt.store(
        Origin::Execute,
        SandboxStoreError::Unavailable(DISCOVERY_DEADLINE.into()),
    );
    assert_eq!(wrong_origin.observation.detail_code, DetailCode::Suppressed);
}

#[test]
fn arbitrary_strings_are_suppressed_and_compatibility_error_is_unchanged() {
    let attempt = LifecycleAttempt::new(Phase::Execution, None);
    for detail in [
        "private-sentinel".to_string(),
        "https://user:private-sentinel@example.invalid/?token=private-sentinel".into(),
        "private-sentinel\n秘密\r\n{payload: private-sentinel}".into(),
        "private-sentinel".repeat(100_000),
    ] {
        for error in [
            SandboxStoreError::Unavailable(detail.clone()),
            SandboxStoreError::Corrupt(detail),
        ] {
            let failure = attempt.store(Origin::Execute, error.clone());
            assert!(failure.observation.detail_suppressed);
            assert!(!failure.observation.detail_retained);
            assert!(!serde_json::to_string(&failure.observation)
                .unwrap()
                .contains("private-sentinel"));
            assert!(!format!("{failure:?}").contains("private-sentinel"));
            assert_eq!(failure.into_compatibility(), error);
        }
    }
    for (error, class, suppressed) in [
        (
            SandboxStoreError::Conflict,
            CauseClass::StoreConflict,
            false,
        ),
        (
            SandboxStoreError::NotFound,
            CauseClass::DomainRefusal,
            false,
        ),
        (
            SandboxStoreError::InvalidTransaction("private-sentinel"),
            CauseClass::DomainRefusal,
            true,
        ),
    ] {
        let failure = attempt.store(Origin::Claim, error.clone());
        assert_eq!(failure.observation.cause_class, class);
        assert_eq!(failure.observation.detail_suppressed, suppressed);
        assert_eq!(failure.into_compatibility(), error);
    }
}

#[tokio::test(start_paused = true)]
async fn attempts_keep_identity_elapsed_and_bounded_continuations_without_cross_task_state() {
    let a = LifecycleAttempt::new(Phase::Execution, Some(&AgentId("private-a".into())));
    let b = LifecycleAttempt::new(Phase::Execution, Some(&AgentId("private-b".into())));
    let admission = a.clone();
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    for index in 0..=6 {
        a.continuation(index);
        let observed = admission
            .store(Origin::ContinuationRead, SandboxStoreError::Conflict)
            .observation;
        assert_eq!(observed.attempt_id, a.id);
        assert_eq!(observed.continuation_index, index);
        assert_eq!(observed.elapsed_ms, 2000);
        assert_eq!(observed.agent_hash, a.agent_hash);
        let other = b.terminated(false);
        assert_ne!(observed.attempt_id, other.attempt_id);
        assert_ne!(observed.agent_hash, other.agent_hash);
        assert_eq!(other.continuation_index, 0);
    }
    a.continuation(u8::MAX);
    assert_eq!(a.terminated(true).continuation_index, 6);
    assert_eq!(a.terminated(true).cause_class, CauseClass::TaskPanicked);
    assert_eq!(b.terminated(false).cause_class, CauseClass::TaskCancelled);
}

#[test]
fn claim_details_are_closed_and_preserve_the_original_store_error() {
    let attempt = LifecycleAttempt::new(Phase::Execution, None);
    let error = SandboxStoreError::Unavailable("private-claim-detail".into());
    for (detail, code) in [
        (
            AgentWorkClaimDetail::RetryExhausted,
            "claim_retry_exhausted",
        ),
        (AgentWorkClaimDetail::Read, "claim_read_failed"),
        (AgentWorkClaimDetail::Driver, "claim_driver_failed"),
        (
            AgentWorkClaimDetail::PublicationCustody,
            "claim_publication_custody_failed",
        ),
    ] {
        let failure = attempt.claim(AgentWorkClaimError::with_detail(error.clone(), detail));
        let observation = &failure.observation;
        assert_eq!(observation.origin, Origin::Claim);
        assert_eq!(observation.cause_class, CauseClass::StoreUnavailable);
        assert_eq!(observation.detail_code.as_str(), code);
        assert!(observation.detail_retained && !observation.detail_suppressed);
        let json = serde_json::to_string(observation).unwrap();
        assert!(json.contains(code));
        assert!(!json.contains("private-claim-detail"));
        assert!(!format!("{failure:?}").contains("private-claim-detail"));
        assert_eq!(failure.into_compatibility(), error);
    }
    let untyped = attempt.claim(error.into());
    assert_eq!(untyped.observation.detail_code, DetailCode::Suppressed);
    let conflict = attempt.claim(AgentWorkClaimError::with_detail(
        SandboxStoreError::Conflict,
        AgentWorkClaimDetail::Read,
    ));
    assert_eq!(conflict.observation.cause_class, CauseClass::StoreConflict);
    assert_eq!(conflict.observation.detail_code, DetailCode::NotApplicable);
}

#[test]
fn every_closed_combination_fits_the_two_kib_field_budget() {
    let attempt = LifecycleAttempt::new(
        Phase::Reconciliation,
        Some(&AgentId("private".repeat(1000))),
    );
    for phase in [Phase::Discovery, Phase::Execution, Phase::Reconciliation] {
        for origin in [
            Origin::Discovery,
            Origin::OwnershipAcquire,
            Origin::AuthorityScope,
            Origin::Claim,
            Origin::Execute,
            Origin::PostWorkRead,
            Origin::ContinuationRead,
            Origin::Handoff,
            Origin::Reconciliation,
            Origin::Unspecified,
        ] {
            for cause in [
                CauseClass::OwnershipStale,
                CauseClass::OwnershipUnavailable,
                CauseClass::OwnershipInvalid,
                CauseClass::IncompatibleProtocol,
                CauseClass::StoreConflict,
                CauseClass::StoreUnavailable,
                CauseClass::StoredDataInvalid,
                CauseClass::DomainRefusal,
                CauseClass::TaskCancelled,
                CauseClass::TaskPanicked,
            ] {
                for detail in [
                    DetailCode::NotApplicable,
                    DetailCode::Suppressed,
                    DetailCode::DiscoveryDeadline,
                    DetailCode::OwnershipDeadline,
                    DetailCode::ReceiverRevocationUnconfirmed,
                    DetailCode::ClaimRetryExhausted,
                    DetailCode::ClaimReadFailed,
                    DetailCode::ClaimDriverFailed,
                    DetailCode::ClaimPublicationCustodyFailed,
                ] {
                    let mut value = attempt.observe(origin, cause, detail);
                    value.phase = phase;
                    value.elapsed_ms = u64::MAX;
                    value.continuation_index = 6;
                    let mut fields = serde_json::to_value(value).unwrap();
                    fields["message"] =
                        serde_json::json!("agent lifecycle task terminated unexpectedly");
                    assert!(serde_json::to_vec(&fields).unwrap().len() <= 2048);
                }
            }
        }
    }
}
