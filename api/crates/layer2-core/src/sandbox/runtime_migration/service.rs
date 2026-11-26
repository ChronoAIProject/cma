use super::{RuntimeMigrationAuthority, RuntimeMigrationIdentity};
use crate::sandbox::store::{
    matching_custom_runtime_replay, runtime_migration_now_ms, RuntimeMigrationAdmissionBasis,
    SandboxCustomRuntimeMigrationAdmission, SandboxMaintenanceStore, SandboxStore,
    SandboxStoreError,
};
use crate::sandbox::{Sandbox, SandboxCustomRuntimeMigration};
use crate::{Caller, Capability, SandboxId};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeMigrationCommand {
    pub migration: SandboxCustomRuntimeMigration,
    pub authority_id: String,
    pub expected_current_revision: u64,
    pub restart_failed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeMigrationError {
    #[error("forbidden")]
    Forbidden,
    #[error("sandbox not found")]
    NotFound,
    #[error("runtime migration is not currently admitted")]
    NotAdmitted,
    #[error("runtime migration dependency unavailable")]
    Unavailable,
}

pub struct SandboxCustomRuntimeMigrationService {
    sandboxes: Arc<dyn SandboxStore>,
    maintenance: Arc<dyn SandboxMaintenanceStore>,
    authority: Option<Arc<dyn RuntimeMigrationAuthority>>,
    enabled: bool,
}

impl SandboxCustomRuntimeMigrationService {
    pub fn new(
        sandboxes: Arc<dyn SandboxStore>,
        maintenance: Arc<dyn SandboxMaintenanceStore>,
    ) -> Self {
        Self {
            sandboxes,
            maintenance,
            authority: None,
            enabled: false,
        }
    }
    pub fn with_authority(
        mut self,
        authority: Arc<dyn RuntimeMigrationAuthority>,
        enabled: bool,
    ) -> Self {
        self.authority = Some(authority);
        self.enabled = enabled;
        self
    }
    pub async fn request(
        &self,
        caller: &Caller,
        id: &SandboxId,
        command: RuntimeMigrationCommand,
    ) -> Result<Sandbox, RuntimeMigrationError> {
        if !caller.authorization.allows(Capability::ControlAnySandbox) {
            return Err(RuntimeMigrationError::Forbidden);
        }
        if id != &command.migration.origin.sandbox_id {
            return Err(RuntimeMigrationError::NotAdmitted);
        }
        let current = self
            .sandboxes
            .get_sandbox(id)
            .await
            .map_err(error)?
            .ok_or(RuntimeMigrationError::NotFound)?;
        if matching_custom_runtime_replay(
            &current,
            &command.migration,
            &command.authority_id,
            command.expected_current_revision,
            command.restart_failed,
        )
        .map_err(error)?
        {
            return Ok(current);
        }
        if !self.enabled {
            return Err(RuntimeMigrationError::NotAdmitted);
        }
        let authority = self
            .authority
            .as_ref()
            .ok_or(RuntimeMigrationError::NotAdmitted)?;
        let entry = authority.resolve(id).await.map_err(error)?;
        authority
            .policy()
            .validate_entry(&entry, runtime_migration_now_ms().map_err(error)?)
            .map_err(error)?;
        if entry.authority_id != command.authority_id
            || entry.custom_migration.as_ref() != Some(&command.migration)
            || entry.identity != RuntimeMigrationIdentity::capture(&current).map_err(error)?
            || entry.identity.revision != command.expected_current_revision
        {
            return Err(RuntimeMigrationError::NotAdmitted);
        }
        self.maintenance
            .admit_custom_runtime_migration(SandboxCustomRuntimeMigrationAdmission {
                migration: command.migration,
                expected_current_revision: command.expected_current_revision,
                basis: RuntimeMigrationAdmissionBasis {
                    authority_id: entry.authority_id,
                    policy_id: authority.policy().policy_id.clone(),
                    request_key: entry.request_key,
                    identity: entry.identity.clone(),
                    authority_issued_at_ms: entry.issued_at_ms,
                    authority_expires_at_ms: entry.expires_at_ms,
                    approved_target: entry.target_image,
                    source_runtime: entry.identity.runtime,
                    clearance: entry.clearance,
                    admitted_at: crate::now_rfc3339(),
                    previous_request_keys: Vec::new(),
                    earlier_admissions: Vec::new(),
                },
                restart_failed: command.restart_failed,
            })
            .await
            .map_err(error)
    }
}

fn error(error: SandboxStoreError) -> RuntimeMigrationError {
    match error {
        SandboxStoreError::NotFound => RuntimeMigrationError::NotFound,
        SandboxStoreError::Unavailable(_) => RuntimeMigrationError::Unavailable,
        _ => RuntimeMigrationError::NotAdmitted,
    }
}
