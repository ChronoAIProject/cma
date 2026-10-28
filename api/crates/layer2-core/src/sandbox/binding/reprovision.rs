//! Explicit fresh-event replacement of an already retired binding hierarchy.
//! Ordinary binding writes never use this transition. The adapter must atomically
//! retain the receipt/custody and fence the complete original hierarchy.
use super::{BindingState, BindingWork, ProvidedCredential, ProviderBinding};
use crate::sandbox::{
    Sandbox, SandboxStatus, SandboxWorkStep, Workspace, WorkspaceStatus, WorkspaceWorkStep,
};
use crate::{
    profiles::content_digest, seconds_from_rfc3339, CoreError, SandboxId, SubjectId, WorkspaceId,
};
use serde::{Deserialize, Serialize};

/// Readers ship before any writer. Once present, provider submissions must name
/// this exact generation before resolving a transport conversation alias.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingGenerationGuard {
    pub generation: u64,
    pub operation_id: String,
}
impl BindingGenerationGuard {
    pub(super) fn valid(&self, generation: u64) -> bool {
        self.generation == generation && valid_operation_id(&self.operation_id)
    }
}

/// A manager-authenticated ordinary event, pinned in the Gateway execution
/// journal before the preparation call. Arbitrary event JSON is not authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingPreparationIntent {
    pub version: u8,
    pub admission_id: String,
    pub target_id: String,
    pub request_hash: String,
    pub binding_id: String,
    pub owner: SubjectId,
    pub manager: SubjectId,
    pub generation: u64,
    pub binding_revision: u64,
    pub binding_digest: String,
    pub sandbox_digest: String,
    pub workspace_digest: String,
    pub admitted_at: i64,
    pub expires_at: i64,
}
impl BindingPreparationIntent {
    pub fn operation_id(&self) -> Result<String, CoreError> {
        content_digest(&(&self.admission_id, &self.target_id, &self.binding_id))
            .map(|hash| format!("bprep_{hash}"))
            .map_err(|_| invalid())
    }
    pub fn fingerprint(&self) -> Result<String, CoreError> {
        self.validate()?;
        content_digest(self).map_err(|_| invalid())
    }
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.version != 1
            || !bounded_id(&self.admission_id)
            || !bounded_id(&self.target_id)
            || !super::valid_binding_id(&self.binding_id)
            || !bounded_id(self.owner.as_str())
            || !bounded_id(self.manager.as_str())
            || self.generation == 0
            || self.binding_revision == 0
            || [
                &self.request_hash,
                &self.binding_digest,
                &self.sandbox_digest,
                &self.workspace_digest,
            ]
            .into_iter()
            .any(|hash| !valid_hash(hash))
            || self.admitted_at <= 0
            || self.expires_at <= self.admitted_at
            || self.expires_at.saturating_sub(self.admitted_at) > 7 * 86400
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn require_current(&self, now: i64) -> Result<(), CoreError> {
        self.validate()?;
        if self.admitted_at > now || self.expires_at <= now {
            return Err(conflict());
        }
        Ok(())
    }
}

/// One immutable receipt also archives the complete former binding generation.
/// No TTL: event expiry never makes the same operation eligible to allocate again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingPreparationReceipt {
    pub version: u8,
    pub operation_id: String,
    pub intent: BindingPreparationIntent,
    pub previous: ProviderBinding,
    pub reserved: ProviderBinding,
    pub reserved_at: String,
}
impl BindingPreparationReceipt {
    pub fn validate(&self) -> Result<(), CoreError> {
        self.intent.validate()?;
        self.previous.validate().map_err(|_| invalid())?;
        self.reserved.validate().map_err(|_| invalid())?;
        let now = unix_seconds(&self.reserved_at).ok_or_else(invalid)?;
        self.intent.require_current(now)?;
        if self.version != 1
            || self.operation_id != self.intent.operation_id()?
            || self.previous.binding_id != self.intent.binding_id
            || self.previous.owner != self.intent.owner
            || self.previous.manager != self.intent.manager
            || self.previous.generation != self.intent.generation
            || self.previous.revision != self.intent.binding_revision
            || content_digest(&self.previous).map_err(|_| invalid())? != self.intent.binding_digest
            || !eligible_binding(&self.previous)
        {
            return Err(invalid());
        }
        let expected = reserve(
            &self.previous,
            &self.intent,
            self.reserved.sandbox_id.clone(),
            self.reserved.workspace_id.clone(),
            self.reserved_at.clone(),
        )?;
        if self.reserved != expected {
            return Err(invalid());
        }
        Ok(())
    }
    /// Exact replay proves one reservation even after later lifecycle revisions.
    pub fn matches(&self, intent: &BindingPreparationIntent) -> Result<(), CoreError> {
        self.validate()?;
        if &self.intent != intent {
            return Err(conflict());
        }
        Ok(())
    }
}

impl ProviderBinding {
    /// Only a dedicated transaction may persist this result. This does not
    /// retire, mutate, recover or reclaim any original Sandbox/PV/history.
    pub fn prepare_retired_hierarchy(
        &self,
        intent: &BindingPreparationIntent,
        sandbox: &Sandbox,
        workspace: &Workspace,
        credential: &ProvidedCredential,
        at: String,
    ) -> Result<BindingPreparationReceipt, CoreError> {
        self.require_retired_hierarchy(intent, sandbox, workspace, credential, &at)?;
        let receipt = BindingPreparationReceipt {
            version: 1,
            operation_id: intent.operation_id()?,
            intent: intent.clone(),
            previous: self.clone(),
            reserved: reserve(
                self,
                intent,
                SandboxId::generate(),
                WorkspaceId::generate(),
                at.clone(),
            )?,
            reserved_at: at,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub(super) fn require_retired_hierarchy(
        &self,
        intent: &BindingPreparationIntent,
        sandbox: &Sandbox,
        workspace: &Workspace,
        credential: &ProvidedCredential,
        at: &str,
    ) -> Result<(), CoreError> {
        let now = unix_seconds(at).ok_or_else(invalid)?;
        intent.require_current(now)?;
        self.validate().map_err(|_| invalid())?;
        let retired_at = sandbox
            .retired_at
            .as_deref()
            .and_then(unix_seconds)
            .ok_or_else(conflict)?;
        if !eligible_binding(self)
            || intent.binding_id != self.binding_id
            || intent.owner != self.owner
            || intent.manager != self.manager
            || intent.generation != self.generation
            || intent.binding_revision != self.revision
            || intent.binding_digest != content_digest(self).map_err(|_| invalid())?
            || intent.sandbox_digest != content_digest(sandbox).map_err(|_| invalid())?
            || intent.workspace_digest != content_digest(workspace).map_err(|_| invalid())?
            || intent.admitted_at <= retired_at
            || credential.sha256() != self.key_sha256
            || sandbox.id != self.sandbox_id
            || sandbox.owner != self.owner
            || sandbox.status != SandboxStatus::Retired
            || sandbox.runtime.is_some()
            || !matches!(
                sandbox.work.step,
                SandboxWorkStep::None | SandboxWorkStep::WaitForPurge
            )
            || sandbox.work.lease.is_some()
            || sandbox.work.execution.is_some()
            || sandbox.lifecycle.operation.is_some()
            || sandbox.lifecycle.home_recovery.is_some()
            || sandbox.lifecycle.unpublished_recovery.is_some()
            || sandbox.lifecycle.runtime_provision.is_some()
            || sandbox.lifecycle.suspended_runtime.is_some()
            || sandbox.lifecycle.checkpoint_capture.is_some()
            || sandbox.lifecycle.background_checkpoint.is_some()
            || sandbox
                .lifecycle
                .home
                .as_ref()
                .is_none_or(|home| !home.released || home.validate(&sandbox.id).is_err())
            || sandbox.credential.source != self.credential_source()
            || !sandbox.credential.validate()
            || sandbox.credential.pending.is_some()
            || !sandbox.credential.revoke_debt.is_empty()
            || workspace.id != self.workspace_id
            || workspace.sandbox_id != self.sandbox_id
            || workspace.owner != self.owner
            || !matches!(
                workspace.status,
                WorkspaceStatus::Ready | WorkspaceStatus::Removed
            )
            || workspace.work.step != WorkspaceWorkStep::None
            || workspace.work.lease.is_some()
            || workspace.work.execution.is_some()
        {
            return Err(conflict());
        }
        Ok(())
    }

    pub fn require_generation(&self, expected: Option<u64>) -> Result<(), CoreError> {
        if expected.is_some_and(|generation| generation != self.generation)
            || (self.generation_guard.is_some() && expected.is_none())
        {
            return Err(conflict());
        }
        Ok(())
    }
}

fn reserve(
    current: &ProviderBinding,
    intent: &BindingPreparationIntent,
    sandbox_id: SandboxId,
    workspace_id: WorkspaceId,
    at: String,
) -> Result<ProviderBinding, CoreError> {
    if sandbox_id == current.sandbox_id
        || workspace_id == current.workspace_id
        || !sandbox_id.as_str().starts_with("sbx_")
        || !workspace_id.as_str().starts_with("wks_")
    {
        return Err(invalid());
    }
    let mut next = current.clone();
    next.generation = current.generation.checked_add(1).ok_or_else(invalid)?;
    next.sandbox_id = sandbox_id;
    next.workspace_id = workspace_id;
    next.state = BindingState::Provisioning;
    next.work = BindingWork::ProvisionSandbox;
    next.generation_guard = Some(BindingGenerationGuard {
        generation: next.generation,
        operation_id: intent.operation_id()?,
    });
    next.advance_revision(at).map_err(|_| invalid())?;
    Ok(next)
}
fn eligible_binding(binding: &ProviderBinding) -> bool {
    matches!(binding.state, BindingState::Failed { .. })
        && binding.work == BindingWork::Idle
        && binding.existing_agent.is_none()
        && binding.previous.is_none()
        && binding.profile.environment.is_some()
}
pub(super) fn bounded_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-:.".contains(&byte))
}
pub(super) fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn valid_operation_id(value: &str) -> bool {
    value.strip_prefix("bprep_").is_some_and(valid_hash)
}
fn conflict() -> CoreError {
    CoreError::InvalidState("binding preparation authority changed or is not terminal".into())
}
fn invalid() -> CoreError {
    CoreError::InvalidRequest("invalid binding preparation proof".into())
}

fn unix_seconds(at: &str) -> Option<i64> {
    seconds_from_rfc3339(at).and_then(|value| value.try_into().ok())
}
