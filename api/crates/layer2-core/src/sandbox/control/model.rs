use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::SandboxCommandId;
use crate::resource_access::{
    ResourceActor, ResourceAuthorizationAudit, ResourceCapability, ResourceOwner, ResourceTarget,
};
use crate::sandbox::agent_io::AgentAuthorizationRevision;
use crate::{SandboxId, SubjectId};

pub const SANDBOX_CONTROL_SCHEMA_VERSION: u16 = 1;
pub const SANDBOX_CONTROL_QUEUE_SECONDS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxControl {
    Pause,
    Revive,
    Retire,
}

impl SandboxControl {
    /// Sharing management is reserved for the resource owner or owning-org
    /// admin. A recipient's whole-Sandbox management grant cannot retire it.
    pub const fn capability(self) -> ResourceCapability {
        match self {
            Self::Pause | Self::Revive => ResourceCapability::ManageSandbox,
            Self::Retire => ResourceCapability::ManageSharing,
        }
    }

    pub fn fingerprint(self, expected_revision: u64) -> String {
        digest(&("sandbox-control-v1", self, expected_revision))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxControlEnvelope {
    pub command_id: SandboxCommandId,
    pub sandbox_id: SandboxId,
    pub sandbox_created_at: String,
    pub owner: ResourceOwner,
    pub actor: ResourceActor,
    /// Nonsecret exact sign-in incarnation; token refresh cannot change its
    /// idempotency namespace, and another sign-in cannot inherit its commands.
    pub authorization: AgentAuthorizationRevision,
    pub command: SandboxControl,
    pub expected_revision: u64,
    pub request_fingerprint: String,
}

impl SandboxControlEnvelope {
    pub fn actor_subject(&self) -> Result<&SubjectId, SandboxControlError> {
        self.actor
            .ordinary_subject()
            .filter(|subject| valid_id(subject.as_str()))
            .ok_or(SandboxControlError::Corrupt)
    }

    pub fn scope(&self) -> String {
        // Ownership/credential revisions are evidence, not new retry namespaces.
        digest(&(
            "sandbox-control-scope-v1",
            &self.authorization.authority_id,
            &self.actor,
            &self.sandbox_id,
            self.command,
        ))
    }

    pub fn target(&self) -> ResourceTarget {
        ResourceTarget::Sandbox {
            sandbox_id: self.sandbox_id.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), SandboxControlError> {
        self.actor_subject()?;
        let owner = match &self.owner {
            ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
        };
        if !valid_id(self.command_id.as_str())
            || !valid_id(self.sandbox_id.as_str())
            || !valid_id(owner.as_str())
            || !valid_id(&self.authorization.authority_id)
            || self.authorization.policy_revision == 0
            || self.authorization.grant_revision == Some(0)
            || self.expected_revision == 0
            || self.expected_revision > i64::MAX as u64
            || crate::seconds_from_rfc3339(&self.sandbox_created_at).is_none()
            || self.request_fingerprint != self.command.fingerprint(self.expected_revision)
        {
            return Err(SandboxControlError::Corrupt);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxControlDisposition {
    Scheduled,
    AlreadyScheduled,
    AlreadySatisfied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxControlFailure {
    AuthorityRevoked,
    QueueExpired,
    TargetChanged,
    NotEligible,
    ConnectionsRequired,
    CredentialSetupRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxControlStatus {
    Queued,
    /// The lifecycle transition is committed, not necessarily finished. Current
    /// Sandbox status remains the source for actual pause/revival/retirement.
    Applied {
        at: String,
        sandbox_revision: u64,
        disposition: SandboxControlDisposition,
    },
    Rejected {
        at: String,
        reason: SandboxControlFailure,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxControlRecord {
    pub schema_version: u16,
    pub envelope: SandboxControlEnvelope,
    pub key_sha256: String,
    pub admitted_at: String,
    pub queue_deadline: String,
    pub status: SandboxControlStatus,
    pub revision: u64,
    pub admission_audit: ResourceAuthorizationAudit,
    pub execution_audit: Option<ResourceAuthorizationAudit>,
}

impl SandboxControlRecord {
    /// A retry observes the original identity and deadline, even after an
    /// unrelated Sandbox revision or credential-policy refresh. Current resource
    /// authority must be checked by the store before calling this helper.
    pub fn replay(&self, old: Self) -> Result<Self, SandboxControlError> {
        self.validate()?;
        old.validate()?;
        if old.envelope.scope() != self.envelope.scope() || old.key_sha256 != self.key_sha256 {
            return Err(SandboxControlError::Corrupt);
        }
        if old.envelope.request_fingerprint != self.envelope.request_fingerprint
            || old.envelope.owner != self.envelope.owner
            || old.envelope.sandbox_created_at != self.envelope.sandbox_created_at
        {
            return Err(SandboxControlError::IdempotencyConflict);
        }
        Ok(old)
    }

    pub fn validate(&self) -> Result<(), SandboxControlError> {
        self.envelope.validate()?;
        let start =
            crate::seconds_from_rfc3339(&self.admitted_at).ok_or(SandboxControlError::Corrupt)?;
        let deadline = crate::seconds_from_rfc3339(&self.queue_deadline)
            .ok_or(SandboxControlError::Corrupt)?;
        if self.schema_version != SANDBOX_CONTROL_SCHEMA_VERSION
            || !valid_digest(&self.key_sha256)
            || start >= deadline
            || deadline - start > SANDBOX_CONTROL_QUEUE_SECONDS
        {
            return Err(SandboxControlError::Corrupt);
        }
        self.validate_audit(&self.admission_audit)?;
        match &self.status {
            SandboxControlStatus::Queued
                if self.revision == 1 && self.execution_audit.is_none() => {}
            SandboxControlStatus::Applied {
                at,
                sandbox_revision,
                disposition,
            } if self.revision == 2
                && Some(*sandbox_revision)
                    == self.envelope.expected_revision.checked_add(u64::from(
                        *disposition == SandboxControlDisposition::Scheduled,
                    ))
                && *sandbox_revision <= i64::MAX as u64
                && crate::seconds_from_rfc3339(at)
                    .is_some_and(|at| start <= at && at < deadline) =>
            {
                self.validate_audit(
                    self.execution_audit
                        .as_ref()
                        .ok_or(SandboxControlError::Corrupt)?,
                )?;
            }
            SandboxControlStatus::Rejected { at, .. }
                if self.revision == 2
                    && self.execution_audit.is_none()
                    && crate::seconds_from_rfc3339(at).is_some_and(|at| at >= start) => {}
            _ => return Err(SandboxControlError::Corrupt),
        }
        Ok(())
    }

    fn validate_audit(
        &self,
        audit: &ResourceAuthorizationAudit,
    ) -> Result<(), SandboxControlError> {
        audit.validate().map_err(|_| SandboxControlError::Corrupt)?;
        let envelope = &self.envelope;
        if audit.target != envelope.target()
            || audit.sandbox_id != envelope.sandbox_id
            || audit.sandbox_created_at != envelope.sandbox_created_at
            || audit.owner != envelope.owner
            || audit.actor != envelope.actor
            || audit.capability != envelope.command.capability()
        {
            return Err(SandboxControlError::Corrupt);
        }
        Ok(())
    }

    pub fn queued(&self) -> bool {
        self.status == SandboxControlStatus::Queued
    }

    pub fn expired(&self, at: &str) -> Result<bool, SandboxControlError> {
        let now = crate::seconds_from_rfc3339(at).ok_or(SandboxControlError::Invalid)?;
        let deadline = crate::seconds_from_rfc3339(&self.queue_deadline)
            .ok_or(SandboxControlError::Corrupt)?;
        Ok(now >= deadline)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxControlError {
    #[error("model budget refused a new Sandbox commitment: {0}")]
    Budget(crate::budget::BudgetError),
    #[error("invalid Sandbox control request")]
    Invalid,
    #[error("Sandbox command is not available")]
    NotFound,
    #[error("Sandbox command key was used for a different request")]
    IdempotencyConflict,
    #[error("Sandbox command observation changed")]
    Conflict,
    #[error("another Sandbox control is queued")]
    Busy,
    #[error("Sandbox command storage is corrupt")]
    Corrupt,
    #[error("Sandbox command storage is unavailable")]
    Unavailable,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.trim() == id && id.len() <= 256
}
fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn digest(value: &impl Serialize) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("typed Sandbox control tuple"))
    )
}
