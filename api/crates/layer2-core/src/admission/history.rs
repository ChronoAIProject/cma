use super::{AdmissionChange, AdmissionControl, ADMISSION_HISTORY_LIMIT};
use crate::StoreError;

/// One consistent operator view with bounded, newest-first immutable evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdmissionControlRead {
    pub control: AdmissionControl,
    pub history: Vec<AdmissionChange>,
}

impl AdmissionControlRead {
    pub fn validate(&self) -> Result<(), StoreError> {
        self.control.validate()?;
        let invalid = || StoreError::Malformed("admission history is inconsistent".into());
        let expected = self.control.revision.min(ADMISSION_HISTORY_LIMIT as u64) as usize;
        if self.history.len() != expected
            || self.history.first() != self.control.last_change.as_ref()
        {
            return Err(invalid());
        }
        for change in &self.history {
            change.validate()?;
        }
        if self.history.windows(2).any(|pair| {
            pair[1].revision.checked_add(1) != Some(pair[0].revision)
                || pair[0].old_stop != pair[1].new_stop
        }) {
            return Err(invalid());
        }
        Ok(())
    }
}

impl AdmissionChange {
    pub fn validate(&self) -> Result<(), StoreError> {
        AdmissionControl {
            revision: self.revision,
            operator_stop: self.new_stop.clone(),
            last_change: Some(self.clone()),
        }
        .validate()
    }
}
