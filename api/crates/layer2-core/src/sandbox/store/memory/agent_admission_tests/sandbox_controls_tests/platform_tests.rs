//! An explicit Revive control meets the platform admission state like the
//! owner's revive does (contract section 5.1); Pause stays available.
use super::*;
use crate::budget::{
    BudgetError, Closure, ClosureReason, CloudObservationInput, ObservationError,
    PlatformAdmissionState, PlatformSpendLedger, PlatformSpendState,
};

struct Spend(Mutex<Vec<ClosureReason>>);

#[tokio::test]
async fn an_admitted_revive_receipt_is_recoverable_by_key_during_stop() {
    let f = Fixture::new().await;
    let spend = Arc::new(Spend(Mutex::new(vec![])));
    f.service.attach_platform_spend(spend.clone());
    let revision = f.store.lock().sandboxes[&sandbox(4).id].revision;
    let admitted = f
        .admit("owner", SandboxControl::Revive, "retained-revive", revision)
        .await
        .unwrap();
    *spend.0.lock().unwrap() = vec![ClosureReason::SpendStop];
    let before = f.store.lock().clone();
    let replay = f
        .admit("owner", SandboxControl::Revive, "retained-revive", revision)
        .await
        .expect("an exact retry must recover its durable receipt during STOP");
    assert_eq!(replay, admitted);
    assert!(matches!(
        f.admit("owner", SandboxControl::Revive, "new-revive", revision)
            .await,
        Err(SandboxControlServiceError::Budget(
            BudgetError::PlatformExhausted
        ))
    ));
    assert!(matches!(
        f.admit(
            "owner",
            SandboxControl::Revive,
            "retained-revive",
            revision + 1
        )
        .await,
        Err(SandboxControlServiceError::Store(
            SandboxControlError::IdempotencyConflict
        ))
    ));
    let after = f.store.lock().clone();
    assert_eq!(after.sandbox_controls, before.sandbox_controls);
    assert_eq!(after.sandboxes, before.sandboxes);
    assert_eq!(after.sandbox_events, before.sandbox_events);
}

#[tokio::test]
async fn a_revive_replay_during_stop_still_requires_current_resource_authority() {
    let f = Fixture::new().await;
    f.share(ShareMode::OrgRoles, None).await;
    let spend = Arc::new(Spend(Mutex::new(vec![])));
    f.service.attach_platform_spend(spend.clone());
    let revision = f.store.lock().sandboxes[&sandbox(4).id].revision;
    f.admit(
        "recipient",
        SandboxControl::Revive,
        "shared-revive",
        revision,
    )
    .await
    .unwrap();
    *spend.0.lock().unwrap() = vec![ClosureReason::SpendStop];
    *f.membership.0.lock().unwrap() = Err(MembershipError::NotMember);
    assert!(matches!(
        f.admit(
            "recipient",
            SandboxControl::Revive,
            "shared-revive",
            revision
        )
        .await,
        Err(SandboxControlServiceError::Access(_))
    ));
    assert_eq!(f.store.lock().sandbox_controls.len(), 1);
}

#[async_trait::async_trait]
impl PlatformSpendLedger for Spend {
    async fn admission_state(&self, _: u64) -> Result<PlatformAdmissionState, BudgetError> {
        Ok(PlatformAdmissionState {
            configured: true,
            operator_control: crate::budget::OperatorAdmissionState::Open,
            closures: self
                .0
                .lock()
                .unwrap()
                .iter()
                .map(|reason| Closure {
                    reason: *reason,
                    since: 1,
                    settings_revision: 1,
                    committed_at_entry: 0,
                })
                .collect(),
        })
    }

    async fn spend_state(&self, _: u64) -> Result<PlatformSpendState, BudgetError> {
        unreachable!("controls read only the admission state")
    }

    async fn record_cloud_observation(
        &self,
        _: CloudObservationInput,
        _: u64,
    ) -> Result<PlatformSpendState, ObservationError> {
        unreachable!("controls record no observation")
    }
}

#[tokio::test]
async fn a_revive_control_is_refused_under_a_platform_stop_while_pause_stays_available() {
    let f = Fixture::new().await;
    let spend = Arc::new(Spend(Mutex::new(vec![
        ClosureReason::SpendWarning,
        ClosureReason::SpendStop,
    ])));
    f.service.attach_platform_spend(spend.clone());
    let revision = f.store.lock().sandboxes[&sandbox(4).id].revision;
    let pause = f
        .admit("owner", SandboxControl::Pause, "pause", revision)
        .await
        .unwrap();
    let proof = f.proof("owner", &pause).await;
    f.store.apply_sandbox_control(&pause, proof).await.unwrap();
    let before = f.store.lock().clone();
    let pausing = &before.sandboxes[&sandbox(4).id];
    assert_eq!(pausing.status, SandboxStatus::Pausing);
    for (closures, expected) in [
        (
            vec![ClosureReason::SpendStop],
            BudgetError::PlatformExhausted,
        ),
        (
            vec![ClosureReason::MeteringUnavailable],
            BudgetError::Unavailable,
        ),
    ] {
        *spend.0.lock().unwrap() = closures;
        assert!(matches!(
            f.admit("owner", SandboxControl::Revive, "revive", pausing.revision)
                .await,
            Err(SandboxControlServiceError::Budget(reason)) if reason == expected
        ));
    }
    let after = f.store.lock().clone();
    assert_eq!(after.sandbox_controls, before.sandbox_controls);
    assert_eq!(after.sandboxes, before.sandboxes);
    assert_eq!(after.sandbox_events, before.sandbox_events);

    *spend.0.lock().unwrap() = vec![ClosureReason::SpendWarning];
    let revive = f
        .admit("owner", SandboxControl::Revive, "revive", pausing.revision)
        .await
        .unwrap();
    let proof = f.proof("owner", &revive).await;
    let applied = f.store.apply_sandbox_control(&revive, proof).await.unwrap();
    assert!(matches!(
        applied.status,
        SandboxControlStatus::Applied { .. }
    ));
}

#[tokio::test]
async fn operator_stop_preserves_exact_revive_replay_and_pause_but_refuses_fresh_revive() {
    use crate::admission::AdmissionControlStore;
    let f = Fixture::new().await;
    f.service.attach_platform_spend(f.store.clone());
    let revision = f.store.lock().sandboxes[&sandbox(4).id].revision;
    let accepted = f
        .admit("owner", SandboxControl::Revive, "accepted-revive", revision)
        .await
        .unwrap();
    f.store
        .change(
            0,
            &SubjectId::new("operator"),
            "contain new work",
            true,
            100,
        )
        .await
        .unwrap();
    assert_eq!(
        f.admit("owner", SandboxControl::Revive, "accepted-revive", revision)
            .await
            .unwrap(),
        accepted
    );
    assert!(matches!(
        f.admit("owner", SandboxControl::Revive, "fresh-revive", revision)
            .await,
        Err(SandboxControlServiceError::AdmissionPaused)
    ));
    assert!(matches!(
        f.admit(
            "owner",
            SandboxControl::Revive,
            "accepted-revive",
            revision + 1
        )
        .await,
        Err(SandboxControlServiceError::Store(
            SandboxControlError::IdempotencyConflict
        ))
    ));
    let proof = f.proof("owner", &accepted).await;
    f.store
        .apply_sandbox_control(&accepted, proof)
        .await
        .unwrap();
    let revision = f.store.lock().sandboxes[&sandbox(4).id].revision;
    let pause = f
        .admit("owner", SandboxControl::Pause, "pause-after-stop", revision)
        .await
        .unwrap();
    let proof = f.proof("owner", &pause).await;
    f.store.apply_sandbox_control(&pause, proof).await.unwrap();
    assert_eq!(f.store.lock().sandbox_controls.len(), 2);
}
