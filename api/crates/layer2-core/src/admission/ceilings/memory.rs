use std::sync::Mutex;

use crate::{ActivityRecord, StoreError, SubjectId};

use super::{
    AdmissionCeilingsChange, AdmissionCeilingsRead, AdmissionCeilingsState, AdmissionCeilingsStore,
};
use crate::admission::ADMISSION_HISTORY_LIMIT;

#[derive(Default)]
struct State {
    state: AdmissionCeilingsState,
    activity: Vec<ActivityRecord>,
    history: Vec<AdmissionCeilingsChange>,
}

/// A single lock mirrors the transaction boundary used by durable adapters.
#[derive(Default)]
pub struct MemoryAdmissionCeilingsStore {
    state: Mutex<State>,
}

impl MemoryAdmissionCeilingsStore {
    pub fn audit_records(&self) -> Result<Vec<ActivityRecord>, StoreError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .activity
            .clone())
    }
}

#[async_trait::async_trait]
impl AdmissionCeilingsStore for MemoryAdmissionCeilingsStore {
    async fn reconcile_pending_homes(
        &self,
        _observed_pending: u64,
        _actor: &SubjectId,
        _reason: &str,
        dry_run: bool,
        _now: u64,
    ) -> Result<crate::admission::PendingHomesReconciliation, StoreError> {
        // This policy-only adapter has no activated Launch receipt ledger.
        Ok(crate::admission::PendingHomesReconciliation {
            dry_run,
            ..Default::default()
        })
    }
    async fn read(&self) -> Result<AdmissionCeilingsState, StoreError> {
        Ok(self.state.lock().map_err(|_| unavailable())?.state.clone())
    }

    async fn read_with_history(&self) -> Result<AdmissionCeilingsRead, StoreError> {
        let state = self.state.lock().map_err(|_| unavailable())?;
        let read = AdmissionCeilingsRead {
            state: state.state.clone(),
            effective_limits: state.state.ceilings.clone(),
            history: state
                .history
                .iter()
                .rev()
                .take(ADMISSION_HISTORY_LIMIT)
                .cloned()
                .collect(),
            ..Default::default()
        };
        read.validate()?;
        Ok(read)
    }

    async fn replace(
        &self,
        expected_revision: u64,
        actor: &SubjectId,
        reason: &str,
        ceilings: super::AdmissionCeilings,
        now: u64,
    ) -> Result<AdmissionCeilingsState, StoreError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if let Some(next) = state
            .state
            .changed(expected_revision, actor, reason, ceilings, now)?
        {
            let change = next.last_change.as_ref().ok_or_else(unavailable)?;
            let record = change.activity_record()?;
            state.history.push(change.clone());
            state.activity.push(record);
            state.state = next;
        }
        Ok(state.state.clone())
    }
}

fn unavailable() -> StoreError {
    StoreError::Unavailable("admission ceilings memory store is unavailable".into())
}
