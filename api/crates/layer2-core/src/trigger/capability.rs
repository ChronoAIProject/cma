use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{
    LaunchId, RepositoryIdentity, RepositoryOperationClass, ScopeClass, TriggerValidationError,
};
use crate::{SessionCredentialAuthority, SessionId, SubjectId};

pub const CAPABILITY_POLICY_VERSION: u32 = 1;
pub const CAPABILITY_MAX_LIFETIME_SECONDS: u64 = 60;
pub const CAPABILITY_MAX_CLOCK_SKEW_SECONDS: u64 = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedAccount {
    pub account_id: u64,
    pub account_node_id: String,
    pub login: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Active,
    Inactive,
    Disconnected,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionSnapshot {
    pub service_id: String,
    pub service_state_version: i64,
    pub backing_key_id: String,
    pub account: ConnectedAccount,
    pub state: ConnectionState,
    #[serde(default)]
    pub granted_scopes: Vec<String>,
    #[serde(default)]
    pub satisfied_scope_classes: Vec<ScopeClass>,
    pub consent_observed_at: String,
    pub preflight_observed_at: String,
}

impl ConnectionSnapshot {
    pub fn validate_for(&self, required: &[ScopeClass]) -> Result<(), TriggerValidationError> {
        if self.service_id.trim().is_empty()
            || self.service_state_version < 0
            || self.backing_key_id.trim().is_empty()
            || self.account.account_id == 0
            || self.account.account_node_id.trim().is_empty()
            || self.account.login.trim().is_empty()
        {
            return Err(TriggerValidationError::field(
                "connection",
                "is missing authoritative identity or generation data",
            ));
        }
        match self.state {
            ConnectionState::Active => {}
            ConnectionState::Inactive => {
                return Err(TriggerValidationError::field(
                    "connection",
                    "the selected service is inactive",
                ))
            }
            ConnectionState::Disconnected | ConnectionState::Revoked | ConnectionState::Expired => {
                return Err(TriggerValidationError::field(
                    "connection",
                    "the selected service must be reconnected",
                ))
            }
        }
        let satisfied: BTreeSet<_> = self.satisfied_scope_classes.iter().copied().collect();
        if let Some(missing) = required.iter().find(|scope| !satisfied.contains(scope)) {
            return Err(TriggerValidationError::field(
                "connection.granted_scopes",
                format!("does not satisfy required scope class {missing:?}"),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationState {
    Available,
    Rehydrating,
    OwnerCredentialUnavailable,
    AuthRequired,
    ServiceInactive,
    Revoked,
    Expired,
    AuthorityDrift,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityProvenance {
    pub subject: SubjectId,
    pub subject_token_generation: u64,
    pub service_id: String,
    pub service_state_version: i64,
    pub backing_key_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_credential_authority: Option<SessionCredentialAuthority>,
    pub state: DelegationState,
    pub observed_at: String,
}

impl AuthorityProvenance {
    pub fn detects_drift(&self, connection: &ConnectionSnapshot) -> bool {
        self.service_id != connection.service_id
            || self.service_state_version != connection.service_state_version
            || self.backing_key_id != connection.backing_key_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityOperation {
    Repository(RepositoryOperationClass),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRevocationState {
    Active,
    RevocationPending,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchCapability {
    pub version: u32,
    pub launch_id: LaunchId,
    pub session_id: SessionId,
    pub runtime_audience: String,
    pub service_id: String,
    pub upstream: RepositoryIdentity,
    pub fork: RepositoryIdentity,
    pub allowed_ref: String,
    #[serde(default)]
    pub operations: Vec<CapabilityOperation>,
    pub generation: u64,
    pub fence: u64,
    pub not_before_epoch_seconds: u64,
    pub expires_at_epoch_seconds: u64,
    pub issuer: String,
    pub audience: String,
    pub signature_key_id: String,
    pub numeric_binding_observed_at: String,
    pub revocation: CapabilityRevocationState,
}

impl LaunchCapability {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.version != CAPABILITY_POLICY_VERSION {
            return Err(TriggerValidationError::field(
                "capability.version",
                "is not supported",
            ));
        }
        if self.generation == 0 || self.fence == 0 {
            return Err(TriggerValidationError::field(
                "capability.generation",
                "generation and fence must be non-zero",
            ));
        }
        let lifetime = self
            .expires_at_epoch_seconds
            .checked_sub(self.not_before_epoch_seconds)
            .ok_or_else(|| {
                TriggerValidationError::field("capability.expires_at", "must be after not_before")
            })?;
        if lifetime == 0 || lifetime > CAPABILITY_MAX_LIFETIME_SECONDS {
            return Err(TriggerValidationError::field(
                "capability.expires_at",
                format!("lifetime must not exceed {CAPABILITY_MAX_LIFETIME_SECONDS} seconds"),
            ));
        }
        for (field, value) in [
            (
                "capability.runtime_audience",
                self.runtime_audience.as_str(),
            ),
            ("capability.service_id", self.service_id.as_str()),
            ("capability.allowed_ref", self.allowed_ref.as_str()),
            ("capability.issuer", self.issuer.as_str()),
            ("capability.audience", self.audience.as_str()),
            (
                "capability.signature_key_id",
                self.signature_key_id.as_str(),
            ),
        ] {
            if value.trim().is_empty() || value.len() > 512 {
                return Err(TriggerValidationError::field(field, "is not usable"));
            }
        }
        self.upstream.validate_public_upstream()?;
        if self.fork.repository_id == 0
            || self.fork.owner_id == 0
            || self.fork.fork_parent_id != Some(self.upstream.repository_id)
            || self.fork.fork_source_id
                != Some(
                    self.upstream
                        .fork_source_id
                        .unwrap_or(self.upstream.repository_id),
                )
        {
            return Err(TriggerValidationError::field(
                "capability.fork",
                "does not match the upstream fork network",
            ));
        }
        super::revision::validate_ref("capability.allowed_ref", &self.allowed_ref)?;
        let operations: BTreeSet<_> = self.operations.iter().copied().collect();
        if operations.is_empty() || operations.len() != self.operations.len() {
            return Err(TriggerValidationError::field(
                "capability.operations",
                "must be non-empty and contain no duplicates",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityRefusal {
    MissingPolicy,
    UnknownVersion,
    Expired,
    StaleGeneration,
    Revoked,
    MalformedRequest,
    ResourceMismatch,
    OperationDenied,
    AudienceMismatch,
    SignatureInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachStatus {
    pub service_id: String,
    pub state: CapabilityRevocationState,
    pub policy_generation: u64,
    #[serde(default)]
    pub policy_fence: u64,
    #[serde(default)]
    pub runtime_audience: String,
    pub expires_at_epoch_seconds: u64,
    #[serde(default)]
    pub numeric_binding_observed_at: String,
    #[serde(default)]
    pub refreshed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_requested_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revocation_effective_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refusal: Option<CapabilityRefusal>,
    pub tombstone_acknowledged: bool,
    pub agent_key_revocation_confirmed: bool,
}

impl ReachStatus {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.service_id.trim().is_empty()
            || self.service_id.len() > 512
            || self.policy_generation == 0
            || self.policy_fence == 0
            || self.runtime_audience.trim().is_empty()
            || self.runtime_audience.len() > 512
            || self.expires_at_epoch_seconds == 0
            || self.numeric_binding_observed_at.trim().is_empty()
            || self.refreshed_at.trim().is_empty()
        {
            return Err(TriggerValidationError::field(
                "reach",
                "is missing its exact policy generation, fence, runtime, or observation",
            ));
        }
        match self.state {
            CapabilityRevocationState::Active | CapabilityRevocationState::Expired
                if self.revocation_requested_at.is_some()
                    || self.revocation_effective_at.is_some()
                    || self.tombstone_acknowledged
                    || self.agent_key_revocation_confirmed =>
            {
                return Err(TriggerValidationError::field(
                    "reach.state",
                    "cannot carry revocation evidence before revocation is requested",
                ));
            }
            CapabilityRevocationState::RevocationPending
                if self.revocation_requested_at.is_none()
                    || self.revocation_effective_at.is_some()
                    || self.tombstone_acknowledged
                    || self.agent_key_revocation_confirmed =>
            {
                return Err(TriggerValidationError::field(
                    "reach.state",
                    "must remain pending without an effective acknowledgement",
                ));
            }
            CapabilityRevocationState::Revoked
                if self.revocation_requested_at.is_none()
                    || self.revocation_effective_at.is_none()
                    || (!self.tombstone_acknowledged && !self.agent_key_revocation_confirmed) =>
            {
                return Err(TriggerValidationError::field(
                    "reach.state",
                    "cannot report revoked without a positive enforcement acknowledgement",
                ));
            }
            _ => {}
        }
        Ok(())
    }
}
