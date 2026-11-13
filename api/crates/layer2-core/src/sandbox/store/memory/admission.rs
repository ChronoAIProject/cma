//! The in-memory store uses the same atomic control/admission boundary.

use super::*;
use crate::admission::{
    AdmissionCause, AdmissionControl, AdmissionControlRead, AdmissionControlStore,
    ADMISSION_HISTORY_LIMIT,
};

#[async_trait]
impl AdmissionControlStore for InMemorySandboxStore {
    async fn read(&self) -> Result<AdmissionControl, crate::StoreError> {
        Ok(self.lock().admission_control.clone())
    }

    async fn read_with_history(&self) -> Result<AdmissionControlRead, crate::StoreError> {
        let state = self.lock();
        let read = AdmissionControlRead {
            control: state.admission_control.clone(),
            history: state
                .admission_history
                .iter()
                .rev()
                .take(ADMISSION_HISTORY_LIMIT)
                .cloned()
                .collect(),
        };
        read.validate()?;
        Ok(read)
    }

    async fn change(
        &self,
        expected_revision: u64,
        actor: &SubjectId,
        reason: &str,
        stopped: bool,
        now: u64,
    ) -> Result<AdmissionControl, crate::StoreError> {
        let mut state = self.lock();
        if let Some(next) =
            state
                .admission_control
                .changed(expected_revision, actor, reason, stopped, now)?
        {
            let change = next
                .last_change
                .as_ref()
                .expect("a change carries evidence");
            let audit = change.activity_record()?;
            state.admission_history.push(change.clone());
            state.admission_audit.push(audit);
            state.admission_control = next;
        }
        Ok(state.admission_control.clone())
    }
}

pub(super) fn require_operator_open(
    state: &State,
    cause: AdmissionCause,
) -> Result<(), SandboxStoreError> {
    if cause == AdmissionCause::Fresh && state.admission_control.operator_stop.is_some() {
        return Err(SandboxStoreError::AdmissionPaused);
    }
    Ok(())
}

#[async_trait]
impl crate::budget::PlatformSpendLedger for InMemorySandboxStore {
    async fn admission_state(
        &self,
        _: u64,
    ) -> Result<crate::budget::PlatformAdmissionState, crate::budget::BudgetError> {
        Ok(crate::budget::PlatformAdmissionState {
            operator_control: self.lock().admission_control.operator_stop.clone().into(),
            ..Default::default()
        })
    }

    async fn spend_state(
        &self,
        _: u64,
    ) -> Result<crate::budget::PlatformSpendState, crate::budget::BudgetError> {
        Ok(crate::budget::PlatformSpendState {
            metering_enabled: false,
            settings_revision: 0,
            levels: None,
            committed: None,
            ledger: None,
        })
    }

    async fn record_cloud_observation(
        &self,
        _: crate::budget::CloudObservationInput,
        _: u64,
    ) -> Result<crate::budget::PlatformSpendState, crate::budget::ObservationError> {
        Err(crate::budget::ObservationError::Invalid)
    }
}
