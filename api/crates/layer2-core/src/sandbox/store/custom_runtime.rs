//! Atomic operator admission for retained-home custom image migrations.
use serde::{Deserialize, Serialize};

use super::{RuntimeActivityProof, SandboxMaintenanceApplication, SandboxStoreError};
use crate::sandbox::{
    Agent, RuntimeBinding, Sandbox, SandboxCustomRuntimeMigration, SandboxEventKind,
    SandboxRotationIdentity, SandboxStatus,
};

const MAX_RETAINED_REQUEST_KEYS: usize = 128;

/// A Paused admission still checks every current child, command and session.
/// Running admission additionally carries qualified, complete quiet evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeMigrationClearance {
    Paused,
    Observed { proof: Box<RuntimeActivityProof> },
}

/// Retained in the same aggregate write that starts ImageRotation. This is
/// permanent replay authority, not a reusable permission to start future work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMigrationAdmissionBasis {
    pub authority_id: String,
    pub policy_id: String,
    pub request_key: String,
    pub identity: crate::sandbox::RuntimeMigrationIdentity,
    pub authority_issued_at_ms: i64,
    pub authority_expires_at_ms: i64,
    pub approved_target: String,
    pub source_runtime: Option<RuntimeBinding>,
    pub clearance: RuntimeMigrationClearance,
    pub admitted_at: String,
    pub previous_request_keys: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub earlier_admissions: Vec<RuntimeMigrationAdmissionBasis>,
}

/// The service resolves this internally from qualified operator configuration.
/// HTTP accepts expected identity guards, never this authority or a quiet claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxCustomRuntimeMigrationAdmission {
    pub migration: SandboxCustomRuntimeMigration,
    pub expected_current_revision: u64,
    pub basis: RuntimeMigrationAdmissionBasis,
    pub restart_failed: bool,
}

impl SandboxCustomRuntimeMigrationAdmission {
    pub fn sandbox_id(&self) -> &crate::SandboxId {
        &self.migration.origin.sandbox_id
    }

    /// Exact read-only replay is resolved before requiring a live bundle. An
    /// explicit restart always takes a new activity admission without retargeting.
    pub fn is_replay(&self, sandbox: &Sandbox) -> Result<bool, SandboxStoreError> {
        let matched = matching_custom_runtime_replay(
            sandbox,
            &self.migration,
            &self.basis.authority_id,
            self.expected_current_revision,
            self.restart_failed,
        )?;
        if !matched {
            return Ok(false);
        }
        let basis = sandbox
            .lifecycle
            .custom_runtime
            .as_ref()
            .and_then(|state| state.admission_basis.as_ref())
            .ok_or_else(invalid)?;
        let original = std::iter::once(basis.as_ref())
            .chain(basis.earlier_admissions.iter())
            .find(|basis| {
                basis.authority_id == self.basis.authority_id
                    && basis.identity.revision == self.expected_current_revision
            })
            .ok_or_else(invalid)?;
        if original.policy_id != self.basis.policy_id
            || original.request_key != self.basis.request_key
            || original.identity != self.basis.identity
            || original.approved_target != self.basis.approved_target
            || original.source_runtime != self.basis.source_runtime
            || original.clearance != self.basis.clearance
            || original.authority_issued_at_ms != self.basis.authority_issued_at_ms
            || original.authority_expires_at_ms != self.basis.authority_expires_at_ms
        {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(true)
    }
}

pub fn matching_custom_runtime_replay(
    sandbox: &Sandbox,
    request: &SandboxCustomRuntimeMigration,
    authority_id: &str,
    expected_current_revision: u64,
    restart_failed: bool,
) -> Result<bool, SandboxStoreError> {
    sandbox.validate().map_err(|_| invalid())?;
    let Some(state) = sandbox.lifecycle.custom_runtime.as_ref() else {
        return Ok(false);
    };
    let basis = state.admission_basis.as_ref().ok_or_else(invalid)?;
    if state.migration.request_key == request.request_key {
        if state.migration != *request {
            return Err(SandboxStoreError::Conflict);
        }
        if restart_failed {
            return Ok(false);
        }
        let accepted = std::iter::once(basis.as_ref())
            .chain(basis.earlier_admissions.iter())
            .any(|basis| {
                basis.authority_id == authority_id
                    && basis.identity.revision == expected_current_revision
            });
        if !accepted {
            return Err(SandboxStoreError::Conflict);
        }
        return Ok(true);
    }
    if basis.previous_request_keys.contains(&request.request_key) {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(false)
}

/// Pure part of admission. Adapters compare the original activity proof and
/// fence all source authority before persisting this application atomically.
#[doc(hidden)]
pub fn apply_custom_runtime_migration(
    current: &Sandbox,
    agents: &[Agent],
    admission: &SandboxCustomRuntimeMigrationAdmission,
) -> Result<SandboxMaintenanceApplication, SandboxStoreError> {
    if admission.is_replay(current)? {
        return Ok(SandboxMaintenanceApplication::Complete(
            super::SandboxMaintenanceOutcome::AlreadyScheduled,
        ));
    }
    let mut basis = admission.basis.clone();
    basis.validate_at(runtime_migration_now_ms()?)?;
    if current.revision != admission.expected_current_revision
        || admission.sandbox_id() != &current.id
        || basis.identity != crate::sandbox::RuntimeMigrationIdentity::capture(current)?
        || basis.request_key != admission.migration.request_key
        || basis.approved_target != admission.migration.target_image
        || !valid_label(&basis.authority_id)
        || !valid_label(&basis.policy_id)
        || crate::seconds_from_rfc3339(&basis.admitted_at).is_none()
        || current.retained_runtime_binding() != basis.source_runtime.as_ref()
        || !crate::sandbox::runtime_update_agents_are_idle(agents)
    {
        return Err(SandboxStoreError::Conflict);
    }
    if matches!(basis.clearance, RuntimeMigrationClearance::Paused)
        && (current.status != SandboxStatus::Paused || admission.restart_failed)
    {
        return Err(invalid());
    }
    let existing = current.lifecycle.custom_runtime.as_ref();
    if admission.restart_failed
        && existing.is_none_or(|state| {
            state.migration != admission.migration
                || state.completed_at.is_some()
                || current
                    .runtime_update_request()
                    .is_none_or(|request| request.failure.is_none())
        })
    {
        return Err(SandboxStoreError::Conflict);
    }
    basis.earlier_admissions.clear();
    if admission.restart_failed {
        let prior = existing
            .and_then(|state| state.admission_basis.as_ref())
            .ok_or_else(invalid)?;
        basis.earlier_admissions = prior.earlier_admissions.clone();
        let mut last = (**prior).clone();
        last.earlier_admissions.clear();
        basis.earlier_admissions.push(last);
        if basis.earlier_admissions.len() > 8 {
            return Err(invalid());
        }
    }
    // History comes only from durable authority, never from the supplied bundle.
    basis.previous_request_keys = existing
        .and_then(|state| state.admission_basis.as_ref())
        .map(|basis| basis.previous_request_keys.clone())
        .unwrap_or_default();
    if let Some(state) = existing {
        if state.migration.request_key != admission.migration.request_key {
            basis
                .previous_request_keys
                .push(state.migration.request_key.clone());
        }
    }
    if basis.previous_request_keys.len() > MAX_RETAINED_REQUEST_KEYS
        || basis
            .previous_request_keys
            .contains(&admission.migration.request_key)
    {
        return Err(invalid());
    }
    if serde_json::to_vec(&basis).map_err(|_| invalid())?.len() > 4 * 1024 * 1024 {
        return Err(invalid());
    }
    let mut next = current.clone();
    if !next
        .request_custom_runtime_migration(
            admission.migration.clone(),
            &basis.approved_target,
            basis.admitted_at.clone(),
        )
        .map_err(|_| invalid())?
    {
        return Err(invalid());
    }
    if !admission.restart_failed {
        let identity = SandboxRotationIdentity::capture(current, agents).map_err(|_| invalid())?;
        next.begin_image_rotation(
            basis.approved_target.clone(),
            identity,
            basis.admitted_at.clone(),
        )
        .map_err(|_| invalid())?;
    }
    // Queue and start are one visible mutation and one CAS, with no stale
    // clearance waiting in a queue. The model request keeps its original guard.
    next.revision = current.revision.checked_add(1).ok_or_else(invalid)?;
    next.lifecycle
        .custom_runtime
        .as_mut()
        .ok_or_else(invalid)?
        .admission_basis = Some(Box::new(basis));
    next.validate().map_err(|_| invalid())?;
    Ok(SandboxMaintenanceApplication::Persist {
        expected_revision: current.revision,
        event: SandboxEventKind::StatusChanged {
            from: current.status,
            to: next.status,
        },
        sandbox: Box::new(next),
    })
}

pub(crate) fn valid_label(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

/// Pure scoped rotation transition, after the adapter's trusted policy and
/// complete transactional activity checks. Preserve its verified basis atomically.
pub fn apply_scoped_image_rotation(
    current: &Sandbox,
    agents: &[Agent],
    request: &super::SandboxImageRotationRequest,
    basis: &RuntimeMigrationAdmissionBasis,
) -> Result<SandboxMaintenanceApplication, SandboxStoreError> {
    basis.validate_at(runtime_migration_now_ms()?)?;
    if basis.identity != crate::sandbox::RuntimeMigrationIdentity::capture(current)?
        || basis.approved_target != request.expected_runtime_version
        || (matches!(basis.clearance, RuntimeMigrationClearance::Paused)
            && current.status != SandboxStatus::Paused)
    {
        return Err(invalid());
    }
    let mut application = super::apply_image_rotation(current, agents, request)?;
    if let SandboxMaintenanceApplication::Persist { sandbox, .. } = &mut application {
        sandbox.lifecycle.runtime_migration_admission = Some(Box::new(basis.clone()));
    }
    Ok(application)
}

const fn invalid() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("custom runtime admission is invalid")
}

/// Real admission time; a bundle timestamp never chooses its own freshness.
pub fn runtime_migration_now_ms() -> Result<i64, SandboxStoreError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .ok_or_else(invalid)
}

impl RuntimeMigrationAdmissionBasis {
    pub fn validate_at(&self, now_ms: i64) -> Result<(), SandboxStoreError> {
        if self.authority_issued_at_ms > now_ms
            || self.authority_expires_at_ms < now_ms
            || self.authority_expires_at_ms < self.authority_issued_at_ms
            || self
                .authority_expires_at_ms
                .saturating_sub(self.authority_issued_at_ms)
                > 30_000
        {
            return Err(invalid());
        }
        if let RuntimeMigrationClearance::Observed { proof } = &self.clearance {
            proof.validate_at(now_ms)?;
        }
        Ok(())
    }
}
