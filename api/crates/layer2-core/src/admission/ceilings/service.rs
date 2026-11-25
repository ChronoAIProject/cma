use std::sync::Arc;

use crate::{Caller, Capability, CoreError, StoreError};

use super::{AdmissionCeilingsRead, AdmissionCeilingsState, AdmissionCeilingsStore};

pub struct AdmissionCeilingsService {
    store: Arc<dyn AdmissionCeilingsStore>,
    writes_enabled: bool,
}

impl AdmissionCeilingsService {
    pub fn new(store: Arc<dyn AdmissionCeilingsStore>) -> Self {
        Self {
            store,
            writes_enabled: false,
        }
    }

    /// Enable after every serving and recovery process understands capacity enforcement.
    pub fn with_writes_enabled(mut self, enabled: bool) -> Self {
        self.writes_enabled = enabled;
        self
    }

    pub fn writes_enabled(&self) -> bool {
        self.writes_enabled
    }

    pub async fn read(&self, caller: &Caller) -> Result<AdmissionCeilingsState, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        self.store.read().await.map_err(store_failed)
    }

    pub async fn read_with_history(
        &self,
        caller: &Caller,
    ) -> Result<AdmissionCeilingsRead, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        self.store.read_with_history().await.map_err(store_failed)
    }

    pub async fn replace(
        &self,
        caller: &Caller,
        expected_revision: u64,
        reason: &str,
        ceilings: super::AdmissionCeilings,
    ) -> Result<AdmissionCeilingsState, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        if !self.writes_enabled {
            return Err(CoreError::AdmissionCeilingsWritesUnavailable);
        }
        if !crate::admission::valid_reason(reason) {
            return Err(CoreError::InvalidRequest(format!(
                "reason must contain 1 to {} bytes without control characters",
                crate::admission::MAX_ADMISSION_REASON_BYTES,
            )));
        }
        ceilings
            .validate()
            .map_err(|error| CoreError::InvalidRequest(error.to_string()))?;
        let now = crate::seconds_from_rfc3339(&crate::now_rfc3339()).ok_or_else(|| {
            CoreError::RuntimeUnavailable("admission clock is unavailable".into())
        })?;
        self.store
            .replace(expected_revision, &caller.subject, reason, ceilings, now)
            .await
            .map_err(store_failed)
    }
}

fn store_failed(error: StoreError) -> CoreError {
    match error {
        StoreError::RevisionConflict { expected, actual } => CoreError::InvalidState(format!(
            "admission ceilings changed: expected revision {expected}, found {actual}"
        )),
        error => CoreError::RuntimeUnavailable(error.to_string()),
    }
}

impl AdmissionCeilingsService {
    pub async fn reconcile_pending_homes(
        &self,
        caller: &Caller,
        observed_pending: u64,
        reason: &str,
        dry_run: bool,
    ) -> Result<crate::admission::PendingHomesReconciliation, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        if !self.writes_enabled {
            return Err(CoreError::AdmissionCeilingsWritesUnavailable);
        }
        if !crate::admission::valid_reason(reason) || observed_pending > i64::MAX as u64 {
            return Err(CoreError::InvalidRequest(
                "invalid pending home correction".into(),
            ));
        }
        let now = crate::seconds_from_rfc3339(&crate::now_rfc3339()).ok_or_else(|| {
            CoreError::RuntimeUnavailable("admission clock is unavailable".into())
        })?;
        self.store
            .reconcile_pending_homes(observed_pending, &caller.subject, reason, dry_run, now)
            .await
            .map_err(store_failed)
    }
}
