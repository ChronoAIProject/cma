//! Authorization, auditing, and optimistic concurrency for platform settings.

use std::sync::Arc;

use crate::{
    ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, ActivityTarget, Caller,
    Capability, CoreError, DetailKey, PlatformSettings, StoreError,
};

mod changes;
pub use changes::{PlatformSettingChange, PlatformSettingChanges};

const TRIGGER_CAS_ATTEMPTS: usize = 4;
const SETTINGS_TARGET: &str = "platform_settings";
const TRIGGERS_TARGET: &str = "triggers";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPlatformSettings {
    pub revision: u64,
    pub settings: PlatformSettings,
    pub changes: PlatformSettingChanges,
}

#[async_trait::async_trait]
pub trait PlatformSettingsStore: Send + Sync {
    async fn read(&self) -> Result<StoredPlatformSettings, StoreError>;
    async fn replace(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, StoreError>;

    async fn replace_with_changes(
        &self,
        expected_revision: u64,
        settings: &PlatformSettings,
        _changes: &PlatformSettingChanges,
    ) -> Result<StoredPlatformSettings, StoreError> {
        self.replace(expected_revision, settings).await
    }
}

pub struct PlatformSettingsService {
    store: Arc<dyn PlatformSettingsStore>,
    activity: Arc<ActivityService>,
    model_budget_writes_enabled: bool,
    model_request_budget_writes_enabled: bool,
    platform_spend_writes_enabled: bool,
}

impl PlatformSettingsService {
    pub fn new(store: Arc<dyn PlatformSettingsStore>, activity: Arc<ActivityService>) -> Self {
        Self {
            store,
            activity,
            model_budget_writes_enabled: false,
            model_request_budget_writes_enabled: false,
            platform_spend_writes_enabled: false,
        }
    }

    /// Enable only after compatible serving and rollback readers are established.
    pub fn with_model_budget_writes_enabled(mut self, enabled: bool) -> Self {
        self.model_budget_writes_enabled = enabled;
        self
    }

    pub fn model_budget_writes_enabled(&self) -> bool {
        self.model_budget_writes_enabled
    }

    /// Request policy needs its own compatible rollback reader after the foundation.
    pub fn with_model_request_budget_writes_enabled(mut self, enabled: bool) -> Self {
        self.model_request_budget_writes_enabled = enabled;
        self
    }

    pub fn model_request_budget_writes_enabled(&self) -> bool {
        self.model_budget_writes_enabled && self.model_request_budget_writes_enabled
    }

    /// Cloud spend fields need a later reader baseline than the foundation.
    pub fn with_platform_spend_writes_enabled(mut self, enabled: bool) -> Self {
        self.platform_spend_writes_enabled = enabled;
        self
    }

    pub fn platform_spend_writes_enabled(&self) -> bool {
        self.model_budget_writes_enabled && self.platform_spend_writes_enabled
    }

    /// Read policy for another application service, without opening an
    /// unauthenticated operator entry point.
    pub async fn current(&self) -> Result<StoredPlatformSettings, CoreError> {
        self.store.read().await.map_err(store_failed)
    }

    pub async fn read(&self, caller: &Caller) -> Result<StoredPlatformSettings, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        let stored = self.current().await?;
        self.audit(
            caller,
            ActivityAction::Read,
            SETTINGS_TARGET,
            stored.revision,
            None,
        )
        .await?;
        Ok(stored)
    }

    pub async fn configure_platform(
        &self,
        caller: &Caller,
        expected_revision: u64,
        settings: &PlatformSettings,
    ) -> Result<StoredPlatformSettings, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        if !self.model_budget_writes_enabled && !settings.model_budget.is_empty_disabled() {
            return Err(CoreError::ModelBudgetWritesUnavailable);
        }
        if !self.model_request_budget_writes_enabled()
            && !settings.model_budget.request_models.is_empty()
        {
            return Err(CoreError::ModelBudgetWritesUnavailable);
        }
        if !self.platform_spend_writes_enabled()
            && settings
                .model_budget
                .platform
                .as_ref()
                .is_some_and(|platform| platform.cloud.is_some())
        {
            return Err(CoreError::ModelBudgetWritesUnavailable);
        }
        let current = self.current().await?;
        let mut effective = settings.clone();
        let policy_changed = current.settings.inactivity.auto_pause_enabled
            != settings.inactivity.auto_pause_enabled
            || current.settings.inactivity.pause_after_seconds
                != settings.inactivity.pause_after_seconds;
        if settings.inactivity.auto_pause_enabled {
            effective.inactivity.grace_started_at =
                if policy_changed || current.settings.inactivity.grace_started_at.is_none() {
                    Some(crate::now_rfc3339())
                } else {
                    current.settings.inactivity.grace_started_at.clone()
                };
        } else {
            effective.inactivity.grace_started_at = None;
        }
        effective.validate()?;
        if current.revision != expected_revision {
            return Err(store_failed(StoreError::RevisionConflict {
                expected: expected_revision,
                actual: current.revision,
            }));
        }
        let changes = changes::changes(&current, &effective, caller);
        let stored = self
            .store
            .replace_with_changes(expected_revision, &effective, &changes)
            .await
            .map_err(mutation_failed)?;
        self.audit(
            caller,
            ActivityAction::Configure,
            SETTINGS_TARGET,
            stored.revision,
            Some((DetailKey::Setting, SETTINGS_TARGET)),
        )
        .await?;
        Ok(stored)
    }

    pub async fn pause_triggers(
        &self,
        caller: &Caller,
    ) -> Result<StoredPlatformSettings, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        self.set_trigger_pause(caller, true).await
    }

    pub async fn resume_triggers(
        &self,
        caller: &Caller,
    ) -> Result<StoredPlatformSettings, CoreError> {
        caller.require(Capability::ConfigurePlatform)?;
        self.set_trigger_pause(caller, false).await
    }

    async fn set_trigger_pause(
        &self,
        caller: &Caller,
        paused: bool,
    ) -> Result<StoredPlatformSettings, CoreError> {
        for _ in 0..TRIGGER_CAS_ATTEMPTS {
            let current = self.current().await?;
            if current.settings.triggers.orchestration_paused == paused {
                self.audit_trigger_control(caller, &current, paused).await?;
                return Ok(current);
            }
            let mut settings = current.settings.clone();
            settings.triggers.orchestration_paused = paused;
            let changes = changes::changes(&current, &settings, caller);
            match self
                .store
                .replace_with_changes(current.revision, &settings, &changes)
                .await
            {
                Ok(stored) => {
                    self.audit_trigger_control(caller, &stored, paused).await?;
                    return Ok(stored);
                }
                Err(StoreError::RevisionConflict { .. }) => continue,
                Err(error) => return Err(mutation_failed(error)),
            }
        }
        Err(CoreError::InvalidState(
            "platform settings kept changing while trigger orchestration was updated".into(),
        ))
    }

    async fn audit_trigger_control(
        &self,
        caller: &Caller,
        stored: &StoredPlatformSettings,
        paused: bool,
    ) -> Result<(), CoreError> {
        self.audit(
            caller,
            ActivityAction::Configure,
            TRIGGERS_TARGET,
            stored.revision,
            Some((DetailKey::Status, if paused { "paused" } else { "running" })),
        )
        .await
    }

    async fn audit(
        &self,
        caller: &Caller,
        action: ActivityAction,
        target: &str,
        revision: u64,
        detail: Option<(DetailKey, &str)>,
    ) -> Result<(), CoreError> {
        let mut record = ActivityRecord::new(action, ActivityOutcome::Succeeded)
            .by(caller.subject.clone())
            .on(ActivityTarget::Resource(target.into()));
        record
            .detail
            .record(DetailKey::Revision, revision.to_string());
        if let Some((key, value)) = detail {
            record.detail.record(key, value);
        }
        self.activity
            .record(&record)
            .await
            .map_err(|error| CoreError::RuntimeUnavailable(error.to_string()))
    }
}

fn mutation_failed(error: StoreError) -> CoreError {
    match error {
        StoreError::ReferenceUnavailable(detail) => CoreError::InvalidRequest(detail),
        error => store_failed(error),
    }
}

fn store_failed(error: StoreError) -> CoreError {
    match error {
        StoreError::RevisionConflict { expected, actual } => CoreError::InvalidState(format!(
            "platform settings changed: expected revision {expected}, found {actual}"
        )),
        error => CoreError::RuntimeUnavailable(error.to_string()),
    }
}

#[cfg(test)]
mod tests;
