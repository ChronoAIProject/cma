use std::sync::Mutex;

use crate::{ActivityRecord, StoreError, SubjectId};

use super::{
    AdmissionChange, AdmissionControl, AdmissionControlRead, AdmissionControlStore,
    ADMISSION_HISTORY_LIMIT,
};

#[derive(Default)]
struct State {
    control: AdmissionControl,
    activity: Vec<ActivityRecord>,
    history: Vec<AdmissionChange>,
}

/// A single lock mirrors the transaction boundary used by durable adapters.
#[derive(Default)]
pub struct MemoryAdmissionControlStore {
    state: Mutex<State>,
}

impl MemoryAdmissionControlStore {
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
impl AdmissionControlStore for MemoryAdmissionControlStore {
    async fn read(&self) -> Result<AdmissionControl, StoreError> {
        Ok(self
            .state
            .lock()
            .map_err(|_| unavailable())?
            .control
            .clone())
    }

    async fn read_with_history(&self) -> Result<AdmissionControlRead, StoreError> {
        let state = self.state.lock().map_err(|_| unavailable())?;
        let read = AdmissionControlRead {
            control: state.control.clone(),
            history: state
                .history
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
    ) -> Result<AdmissionControl, StoreError> {
        let mut state = self.state.lock().map_err(|_| unavailable())?;
        if let Some(next) = state
            .control
            .changed(expected_revision, actor, reason, stopped, now)?
        {
            let change = next.last_change.as_ref().ok_or_else(unavailable)?;
            let record = change.activity_record()?;
            state.history.push(change.clone());
            state.activity.push(record);
            state.control = next;
        }
        Ok(state.control.clone())
    }
}

fn unavailable() -> StoreError {
    StoreError::Unavailable("admission control memory store is unavailable".into())
}
