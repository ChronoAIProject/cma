use std::sync::atomic::{AtomicUsize, Ordering};

use layer2_core::budget::{
    BudgetError, CloudObservationInput, ObservationError, OperatorAdmissionState,
    PlatformAdmissionState, PlatformSpendLedger, PlatformSpendState,
};

use super::*;
use crate::auth::{records::RegistrationDecision, AuthMode};

struct Capacity {
    existing: bool,
    open: bool,
    admitted: AtomicUsize,
}

#[async_trait::async_trait]
impl RegistrationCapacity for Capacity {
    async fn existing_subject(&self, _: &SubjectId) -> Result<bool, StoreError> {
        Ok(self.existing)
    }

    async fn admit(&self, _: &SubjectId) -> Result<bool, StoreError> {
        self.admitted.fetch_add(1, Ordering::Relaxed);
        Ok(self.open)
    }

    async fn available(&self) -> Result<bool, StoreError> {
        Ok(self.open)
    }
}

struct UnreadableControl;

#[async_trait::async_trait]
impl PlatformSpendLedger for UnreadableControl {
    async fn admission_state(&self, _: u64) -> Result<PlatformAdmissionState, BudgetError> {
        Ok(PlatformAdmissionState {
            operator_control: OperatorAdmissionState::Unreadable,
            ..Default::default()
        })
    }

    async fn spend_state(&self, _: u64) -> Result<PlatformSpendState, BudgetError> {
        Err(BudgetError::Unavailable)
    }

    async fn record_cloud_observation(
        &self,
        _: CloudObservationInput,
        _: u64,
    ) -> Result<PlatformSpendState, ObservationError> {
        Err(ObservationError::Unavailable)
    }
}

fn capacity(existing: bool, open: bool) -> Arc<Capacity> {
    Arc::new(Capacity {
        existing,
        open,
        admitted: AtomicUsize::new(0),
    })
}

#[tokio::test]
async fn retained_owner_or_registration_replay_bypasses_unreadable_control() {
    let capacity = capacity(true, false);
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "test-owner".into(),
        },
        None,
    )
    .with_platform_spend(Arc::new(UnreadableControl))
    .with_registration_capacity(capacity.clone());
    assert_eq!(
        auth.admit_registration(&SubjectId::new("retained-owner"))
            .await
            .unwrap(),
        RegistrationDecision::Existing
    );
    assert_eq!(capacity.admitted.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn operator_refusal_precedes_registration_capacity_reservation() {
    let capacity = capacity(false, true);
    let auth = AuthState::in_memory(
        AuthMode::Trusted {
            subject: "test-owner".into(),
        },
        None,
    )
    .with_platform_spend(Arc::new(UnreadableControl))
    .with_registration_capacity(capacity.clone());
    assert_eq!(
        auth.admit_registration(&SubjectId::new("new-owner"))
            .await
            .unwrap(),
        RegistrationDecision::Closed
    );
    assert_eq!(capacity.admitted.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn registration_capacity_applies_without_platform_spend_and_updates_options() {
    for open in [false, true] {
        let capacity = capacity(false, open);
        let auth = AuthState::in_memory(
            AuthMode::Trusted {
                subject: "test-owner".into(),
            },
            None,
        )
        .with_registration_capacity(capacity.clone());
        assert_eq!(
            auth.admit_registration(&SubjectId::new("new-owner"))
                .await
                .unwrap(),
            if open {
                RegistrationDecision::Admitted
            } else {
                RegistrationDecision::Closed
            }
        );
        assert_eq!(auth.registration_open().await, open);
        assert_eq!(capacity.admitted.load(Ordering::Relaxed), 1);
    }
}
