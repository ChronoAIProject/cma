use std::sync::{Arc, Mutex};

use super::*;
use crate::budget::{BudgetError, PlatformAdmissionState};
use crate::{
    ActivityAction, ActivityQuery, ActivityRecord, ActivityStore, ActivityTarget, Authorization,
    Capability, Page, StoreError, SubjectId,
};

#[derive(Default)]
struct Ledger {
    observations: Mutex<Vec<CloudObservationInput>>,
}

#[async_trait::async_trait]
impl PlatformSpendLedger for Ledger {
    async fn admission_state(&self, _: u64) -> Result<PlatformAdmissionState, BudgetError> {
        Ok(PlatformAdmissionState::default())
    }

    async fn spend_state(&self, _: u64) -> Result<PlatformSpendState, BudgetError> {
        Ok(PlatformSpendState {
            settings_revision: 7,
            levels: None,
            committed: None,
            ledger: None,
        })
    }

    async fn record_cloud_observation(
        &self,
        input: CloudObservationInput,
        _: u64,
    ) -> Result<PlatformSpendState, ObservationError> {
        self.observations.lock().unwrap().push(input);
        self.spend_state(0)
            .await
            .map_err(|_| ObservationError::Unavailable)
    }
}

#[derive(Default)]
struct Activity {
    records: Mutex<Vec<ActivityRecord>>,
}

#[async_trait::async_trait]
impl ActivityStore for Activity {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }

    async fn list(&self, _: &ActivityQuery) -> Result<Page<ActivityRecord>, StoreError> {
        unreachable!("the spend service never lists activity")
    }
}

fn caller(capabilities: impl IntoIterator<Item = Capability>) -> Caller {
    Caller::new(
        SubjectId::new("operator-a"),
        Authorization::new(capabilities),
    )
}

const INPUT: CloudObservationInput = CloudObservationInput {
    observed_through: 1_700_000_000,
    units: 4_242_424,
};

#[tokio::test]
async fn observations_require_configure_platform_and_the_write_gate() {
    let ledger = Arc::new(Ledger::default());
    let activity = Arc::new(Activity::default());
    let closed = PlatformSpendService::new(ledger.clone(), ActivityService::new(activity.clone()));
    let operator = caller([Capability::ConfigurePlatform]);
    assert_eq!(
        closed.record_cloud_observation(&operator, INPUT).await,
        Err(CoreError::ModelBudgetWritesUnavailable)
    );
    let open = closed.with_writes_enabled(true);
    for denied in [vec![], vec![Capability::ReadAllSandboxes]] {
        assert_eq!(
            open.record_cloud_observation(&caller(denied.clone()), INPUT)
                .await,
            Err(CoreError::Forbidden)
        );
        assert_eq!(open.read(&caller(denied)).await, Err(CoreError::Forbidden));
    }
    assert!(ledger.observations.lock().unwrap().is_empty());
    assert_eq!(open.read(&operator).await.unwrap().settings_revision, 7);
    open.record_cloud_observation(&operator, INPUT)
        .await
        .unwrap();
    assert_eq!(*ledger.observations.lock().unwrap(), vec![INPUT]);
}

#[tokio::test]
async fn observation_audit_records_revision_but_no_units() {
    let activity = Arc::new(Activity::default());
    let service = PlatformSpendService::new(
        Arc::new(Ledger::default()),
        ActivityService::new(activity.clone()),
    )
    .with_writes_enabled(true);
    service
        .record_cloud_observation(&caller([Capability::ConfigurePlatform]), INPUT)
        .await
        .unwrap();
    let records = activity.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].action, ActivityAction::Configure);
    assert_eq!(
        records[0].target,
        Some(ActivityTarget::Resource("platform_spend".into()))
    );
    let audit = serde_json::to_string(&records[0]).unwrap();
    assert!(audit.contains('7'), "{audit}");
    assert!(!audit.contains("4242424"), "{audit}");
    assert!(!audit.contains("1700000000"), "{audit}");
}
