use std::sync::Arc;

use crate::{Caller, Capability, CoreError, StoreError};

use super::{AdmissionControl, AdmissionControlRead, AdmissionControlStore};

pub struct AdmissionControlService {
    store: Arc<dyn AdmissionControlStore>,
    writes_enabled: bool,
}

impl AdmissionControlService {
    pub fn new(store: Arc<dyn AdmissionControlStore>) -> Self {
        Self {
            store,
            writes_enabled: false,
        }
    }

    /// Enable after every serving and recovery process understands the stop.
    pub fn with_writes_enabled(mut self, enabled: bool) -> Self {
        self.writes_enabled = enabled;
        self
    }

    pub fn writes_enabled(&self) -> bool {
        self.writes_enabled
    }

    pub async fn read(&self, caller: &Caller) -> Result<AdmissionControl, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        self.store.read().await.map_err(store_failed)
    }

    pub async fn read_with_history(
        &self,
        caller: &Caller,
    ) -> Result<AdmissionControlRead, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        self.store.read_with_history().await.map_err(store_failed)
    }

    pub async fn stop(
        &self,
        caller: &Caller,
        expected_revision: u64,
        reason: &str,
    ) -> Result<AdmissionControl, CoreError> {
        self.change(caller, expected_revision, reason, true).await
    }

    pub async fn resume(
        &self,
        caller: &Caller,
        expected_revision: u64,
        reason: &str,
    ) -> Result<AdmissionControl, CoreError> {
        self.change(caller, expected_revision, reason, false).await
    }

    async fn change(
        &self,
        caller: &Caller,
        expected_revision: u64,
        reason: &str,
        stopped: bool,
    ) -> Result<AdmissionControl, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        if !self.writes_enabled {
            return Err(CoreError::AdmissionControlWritesUnavailable);
        }
        if !super::valid_reason(reason) {
            return Err(CoreError::InvalidRequest(format!(
                "reason must contain 1 to {} bytes without control characters",
                super::MAX_ADMISSION_REASON_BYTES,
            )));
        }
        let now = crate::seconds_from_rfc3339(&crate::now_rfc3339()).ok_or_else(|| {
            CoreError::RuntimeUnavailable("admission clock is unavailable".into())
        })?;
        self.store
            .change(expected_revision, &caller.subject, reason, stopped, now)
            .await
            .map_err(store_failed)
    }
}

fn store_failed(error: StoreError) -> CoreError {
    match error {
        StoreError::RevisionConflict { expected, actual } => CoreError::InvalidState(format!(
            "admission control changed: expected revision {expected}, found {actual}"
        )),
        error => CoreError::RuntimeUnavailable(error.to_string()),
    }
}
