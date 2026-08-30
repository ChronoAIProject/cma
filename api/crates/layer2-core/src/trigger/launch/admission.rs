use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    AuthorityProvenance, BranchRecord, ConnectionSnapshot, EffectClaim, EffectClaimMode, Launch,
    LaunchEffectKind, RepositoryIdentity, TriggerValidationError, WorkerLease,
};
use crate::DelegationState;

pub const AUTHORITY_ADMISSION_VERSION: u32 = 1;

/// The closed reason for one just-in-time authority decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthorityAdmissionPurpose {
    Effect {
        kind: LaunchEffectKind,
        intent_key: String,
        attempt: u32,
    },
    CapabilityRenewal {
        target_policy_generation: u64,
    },
}

/// Credential-free evidence durably committed before an irreversible effect.
///
/// A snapshot is authorization only for its exact Launch, intent, and worker
/// fence. Provider bearers and backing credential values are never retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityAdmissionSnapshot {
    pub version: u32,
    pub launch_id: super::super::LaunchId,
    pub launch_revision_digest: String,
    pub intent_digest: String,
    pub purpose: AuthorityAdmissionPurpose,
    pub fence: u64,
    pub connection: ConnectionSnapshot,
    pub authority: AuthorityProvenance,
    pub upstream: RepositoryIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork: Option<RepositoryIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<BranchRecord>,
    pub observed_at: String,
}

/// One policy generation reserved and admitted before signing or installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityAuthorityAdmission {
    pub snapshot: AuthorityAdmissionSnapshot,
    pub target_policy_generation: u64,
    pub target_policy_expires_at_epoch_seconds: u64,
}

impl AuthorityAdmissionSnapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn for_effect(
        launch: &Launch,
        claim: &EffectClaim,
        connection: ConnectionSnapshot,
        authority: AuthorityProvenance,
        upstream: RepositoryIdentity,
        fork: Option<RepositoryIdentity>,
        branch: Option<BranchRecord>,
        observed_at: impl Into<String>,
    ) -> Result<Self, TriggerValidationError> {
        let purpose = AuthorityAdmissionPurpose::Effect {
            kind: claim.kind,
            intent_key: claim.intent_key.clone(),
            attempt: claim.attempt,
        };
        let mut snapshot = Self {
            version: AUTHORITY_ADMISSION_VERSION,
            launch_id: launch.id.clone(),
            launch_revision_digest: launch.revision_digest.clone(),
            intent_digest: String::new(),
            purpose,
            fence: claim.fence,
            connection,
            authority,
            upstream,
            fork,
            branch,
            observed_at: observed_at.into(),
        };
        snapshot.intent_digest = snapshot.expected_intent_digest()?;
        snapshot.validate_common(launch)?;
        snapshot.validate_effect(launch, claim)?;
        Ok(snapshot)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn for_capability_renewal(
        launch: &Launch,
        fence: u64,
        target_policy_generation: u64,
        connection: ConnectionSnapshot,
        authority: AuthorityProvenance,
        upstream: RepositoryIdentity,
        fork: RepositoryIdentity,
        branch: BranchRecord,
        observed_at: impl Into<String>,
    ) -> Result<Self, TriggerValidationError> {
        let purpose = AuthorityAdmissionPurpose::CapabilityRenewal {
            target_policy_generation,
        };
        let mut snapshot = Self {
            version: AUTHORITY_ADMISSION_VERSION,
            launch_id: launch.id.clone(),
            launch_revision_digest: launch.revision_digest.clone(),
            intent_digest: String::new(),
            purpose,
            fence,
            connection,
            authority,
            upstream,
            fork: Some(fork),
            branch: Some(branch),
            observed_at: observed_at.into(),
        };
        snapshot.intent_digest = snapshot.expected_intent_digest()?;
        snapshot.validate_common(launch)?;
        snapshot.validate_capability(launch, fence, target_policy_generation)?;
        Ok(snapshot)
    }

    pub fn validate_effect_for(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
    ) -> Result<(), TriggerValidationError> {
        self.validate_common(launch)?;
        self.validate_effect(launch, claim)
    }

    pub fn validate_capability_for(
        &self,
        launch: &Launch,
        fence: u64,
        target_policy_generation: u64,
    ) -> Result<(), TriggerValidationError> {
        self.validate_common(launch)?;
        self.validate_capability(launch, fence, target_policy_generation)
    }

    fn validate_common(&self, launch: &Launch) -> Result<(), TriggerValidationError> {
        if self.version != AUTHORITY_ADMISSION_VERSION
            || self.launch_id != launch.id
            || self.launch_revision_digest != launch.revision_digest
            || self.intent_digest != self.expected_intent_digest()?
            || self.fence == 0
            || crate::seconds_from_rfc3339(&self.observed_at).is_none()
        {
            return Err(TriggerValidationError::field(
                "authority_admission",
                "does not bind this exact Launch, intent, version, and observation",
            ));
        }
        self.connection.validate_for(
            &launch
                .revision
                .content
                .repository_access
                .required_scope_classes,
        )?;
        let retained = launch.connection.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "authority_admission.connection",
                "has no retained Launch connection to revalidate",
            )
        })?;
        if !same_connection_authority(retained, &self.connection)
            || self.authority.subject != launch.owner
            || self.authority.detects_drift(&self.connection)
            || self.authority.state != DelegationState::Available
            || crate::seconds_from_rfc3339(&self.authority.observed_at).is_none()
            || !same_repository_authority(
                &launch.revision.content.upstream.repository,
                &self.upstream,
            )
        {
            return Err(TriggerValidationError::field(
                "authority_admission",
                "contains authority drift or a different numeric repository binding",
            ));
        }
        if let Some(credential) = self.authority.session_credential_authority.as_ref() {
            let Some(session) = launch.session.as_ref() else {
                return Err(TriggerValidationError::field(
                    "authority_admission.session_credential_authority",
                    "has no retained Trigger session",
                ));
            };
            let observed_millis = crate::seconds_from_rfc3339(&self.authority.observed_at)
                .and_then(|seconds| seconds.checked_mul(1_000))
                .ok_or_else(|| {
                    TriggerValidationError::field(
                        "authority_admission.session_credential_authority",
                        "has no valid observation time",
                    )
                })?;
            if credential.session_id != session.session_id
                || !credential.is_valid_at(observed_millis)
            {
                return Err(TriggerValidationError::field(
                    "authority_admission.session_credential_authority",
                    "does not prove a live credential for the retained Trigger session",
                ));
            }
        }
        Ok(())
    }

    fn validate_effect(
        &self,
        launch: &Launch,
        claim: &EffectClaim,
    ) -> Result<(), TriggerValidationError> {
        if claim.mode != EffectClaimMode::Execute
            || self.fence != claim.fence
            || self.purpose
                != (AuthorityAdmissionPurpose::Effect {
                    kind: claim.kind,
                    intent_key: claim.intent_key.clone(),
                    attempt: claim.attempt,
                })
        {
            return Err(TriggerValidationError::field(
                "authority_admission",
                "does not authorize this exact executable effect claim",
            ));
        }
        let (fork_required, branch_required) = match claim.kind {
            LaunchEffectKind::Fork => (false, false),
            LaunchEffectKind::Branch => (true, false),
            LaunchEffectKind::Reach
            | LaunchEffectKind::BranchPublication
            | LaunchEffectKind::PullRequest => (true, true),
            _ => {
                return Err(TriggerValidationError::field(
                    "authority_admission.effect",
                    "is not an irreversible GitHub authority boundary",
                ))
            }
        };
        if claim.kind == LaunchEffectKind::Reach
            && self.authority.session_credential_authority.is_none()
        {
            return Err(TriggerValidationError::field(
                "authority_admission.session_credential_authority",
                "is required before initial Reach admission",
            ));
        }
        self.validate_fork_and_branch(launch, fork_required, branch_required)
    }

    fn validate_capability(
        &self,
        launch: &Launch,
        fence: u64,
        target_policy_generation: u64,
    ) -> Result<(), TriggerValidationError> {
        if self.fence != fence
            || target_policy_generation == 0
            || self.purpose
                != (AuthorityAdmissionPurpose::CapabilityRenewal {
                    target_policy_generation,
                })
        {
            return Err(TriggerValidationError::field(
                "capability_authority_admission",
                "does not authorize this exact policy generation and fence",
            ));
        }
        let credential = self
            .authority
            .session_credential_authority
            .as_ref()
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "capability_authority_admission.session_credential_authority",
                    "is required for policy renewal",
                )
            })?;
        let previous = launch
            .authority
            .as_ref()
            .and_then(|authority| authority.session_credential_authority.as_ref())
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "capability_authority_admission.session_credential_authority",
                    "has no previously admitted credential authority",
                )
            })?;
        if !credential.is_same_or_successor_of(previous) {
            return Err(TriggerValidationError::field(
                "capability_authority_admission.session_credential_authority",
                "is neither the admitted credential nor a valid custody successor",
            ));
        }
        self.validate_fork_and_branch(launch, true, true)
    }

    fn validate_fork_and_branch(
        &self,
        launch: &Launch,
        fork_required: bool,
        branch_required: bool,
    ) -> Result<(), TriggerValidationError> {
        if fork_required {
            let retained = &launch
                .fork
                .as_ref()
                .ok_or_else(|| {
                    TriggerValidationError::field(
                        "authority_admission.fork",
                        "has no retained Launch fork",
                    )
                })?
                .repository;
            let admitted = self.fork.as_ref().ok_or_else(|| {
                TriggerValidationError::field(
                    "authority_admission.fork",
                    "is required for this effect",
                )
            })?;
            if !same_repository_authority(retained, admitted)
                || admitted.owner_id != self.connection.account.account_id
                || admitted.owner_node_id != self.connection.account.account_node_id
            {
                return Err(TriggerValidationError::field(
                    "authority_admission.fork",
                    "does not retain the connected account's numeric fork identity",
                ));
            }
        } else if self.fork.is_some() {
            return Err(TriggerValidationError::field(
                "authority_admission.fork",
                "must be absent before the fork effect",
            ));
        }
        if branch_required {
            let retained = launch.branch.as_ref().ok_or_else(|| {
                TriggerValidationError::field(
                    "authority_admission.branch",
                    "has no retained Launch branch",
                )
            })?;
            let admitted = self.branch.as_ref().ok_or_else(|| {
                TriggerValidationError::field(
                    "authority_admission.branch",
                    "is required for this effect",
                )
            })?;
            let require_retained_head = matches!(
                &self.purpose,
                AuthorityAdmissionPurpose::Effect {
                    kind: LaunchEffectKind::Reach
                        | LaunchEffectKind::BranchPublication
                        | LaunchEffectKind::PullRequest,
                    ..
                }
            );
            if admitted.name != retained.name
                || admitted.expected_head != retained.expected_head
                || admitted.created_at != retained.created_at
                || (require_retained_head && admitted.current_head != retained.current_head)
                || super::super::validate_sha(
                    "authority_admission.branch.current_head",
                    &admitted.current_head,
                )
                .is_err()
            {
                return Err(TriggerValidationError::field(
                    "authority_admission.branch",
                    "does not retain the exact Launch branch identity and current head",
                ));
            }
        } else if self.branch.is_some() {
            return Err(TriggerValidationError::field(
                "authority_admission.branch",
                "must be absent before the branch is durably available",
            ));
        }
        Ok(())
    }

    fn expected_intent_digest(&self) -> Result<String, TriggerValidationError> {
        let bytes = serde_json::to_vec(&(
            self.version,
            &self.launch_id,
            &self.launch_revision_digest,
            &self.purpose,
            self.fence,
        ))
        .map_err(|_| {
            TriggerValidationError::field(
                "authority_admission.intent_digest",
                "cannot be derived from the closed intent",
            )
        })?;
        Ok(Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }
}

impl CapabilityAuthorityAdmission {
    pub fn validate_for(
        &self,
        launch: &Launch,
        lease: &WorkerLease,
    ) -> Result<(), TriggerValidationError> {
        if self.target_policy_generation == 0
            || self.target_policy_expires_at_epoch_seconds == 0
            || self.snapshot.fence != lease.fencing_token
        {
            return Err(TriggerValidationError::field(
                "capability_authority_admission",
                "does not reserve a bounded policy generation on the current fence",
            ));
        }
        self.snapshot.validate_capability_for(
            launch,
            lease.fencing_token,
            self.target_policy_generation,
        )
    }
}

fn same_connection_authority(expected: &ConnectionSnapshot, actual: &ConnectionSnapshot) -> bool {
    expected.service_id == actual.service_id
        && expected.github_catalog_service_id == actual.github_catalog_service_id
        && expected.github_user_service_slug == actual.github_user_service_slug
        && expected.service_state_version == actual.service_state_version
        && expected.backing_key_id == actual.backing_key_id
        && expected.account.account_id == actual.account.account_id
        && expected.account.account_node_id == actual.account.account_node_id
        && expected.granted_scopes.iter().collect::<BTreeSet<_>>()
            == actual.granted_scopes.iter().collect::<BTreeSet<_>>()
        && expected
            .satisfied_scope_classes
            .iter()
            .collect::<BTreeSet<_>>()
            == actual
                .satisfied_scope_classes
                .iter()
                .collect::<BTreeSet<_>>()
}

fn same_repository_authority(expected: &RepositoryIdentity, actual: &RepositoryIdentity) -> bool {
    expected.repository_id == actual.repository_id
        && expected.repository_node_id == actual.repository_node_id
        && expected.owner_id == actual.owner_id
        && expected.owner_node_id == actual.owner_node_id
        && expected.visibility == actual.visibility
        && expected.allow_forking == actual.allow_forking
        && expected.selected_ref == actual.selected_ref
        && expected.fork_parent_id == actual.fork_parent_id
        && expected.fork_source_id == actual.fork_source_id
}

pub fn effect_requires_authority_admission(kind: LaunchEffectKind) -> bool {
    matches!(
        kind,
        LaunchEffectKind::Fork
            | LaunchEffectKind::Branch
            | LaunchEffectKind::Reach
            | LaunchEffectKind::BranchPublication
            | LaunchEffectKind::PullRequest
    )
}

impl Launch {
    /// Commit just-in-time authority under the same lease that owns the effect.
    pub fn record_effect_authority_admission(
        &mut self,
        lease: &WorkerLease,
        claim: &EffectClaim,
        admission: AuthorityAdmissionSnapshot,
        now: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let now = now.into();
        validate_admission_lease(self, lease, &now)?;
        admission.validate_effect_for(self, claim)?;
        let record = self
            .effects
            .iter_mut()
            .find(|record| {
                record.kind == claim.kind
                    && record.intent_key == claim.intent_key
                    && record.attempt == claim.attempt
                    && record.fence == claim.fence
                    && record.state == super::EffectState::Claimed
            })
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "effect_claim",
                    "is stale or no longer owns an executable admission",
                )
            })?;
        if record.authority_admission.is_some() {
            return Err(TriggerValidationError::field(
                "authority_admission",
                "has already been committed for this effect fence",
            ));
        }
        record.authority_admission = Some(admission);
        record.updated_at = now.clone();
        self.updated_at = now;
        Ok(())
    }

    pub fn effect_authority_admission(
        &self,
        claim: &EffectClaim,
    ) -> Result<&AuthorityAdmissionSnapshot, TriggerValidationError> {
        let admission = self
            .effects
            .iter()
            .find(|record| {
                record.kind == claim.kind
                    && record.intent_key == claim.intent_key
                    && record.attempt == claim.attempt
                    && record.fence == claim.fence
                    && record.state == super::EffectState::Claimed
            })
            .and_then(|record| record.authority_admission.as_ref())
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "authority_admission",
                    "was not durably committed for this effect fence",
                )
            })?;
        admission.validate_effect_for(self, claim)?;
        Ok(admission)
    }

    /// Return the immutable admission retained for a reconciliation claim.
    ///
    /// This is evidence that the original worker may have crossed its external
    /// boundary. It is deliberately separate from
    /// [`Self::effect_authority_admission_at`]: the returned snapshot retains the
    /// original fence and cannot authorize a write under the replacement
    /// worker's lease. A reconciler may use it only to bind read-only provider or
    /// runtime observations to the original intent.
    pub fn historical_effect_authority_admission(
        &self,
        claim: &EffectClaim,
    ) -> Result<&AuthorityAdmissionSnapshot, TriggerValidationError> {
        if claim.mode != EffectClaimMode::Reconcile {
            return Err(TriggerValidationError::field(
                "effect_claim.mode",
                "must be reconciliation before historical admission is read",
            ));
        }
        let admission = self
            .effects
            .iter()
            .find(|record| {
                record.kind == claim.kind
                    && record.intent_key == claim.intent_key
                    && record.attempt == claim.attempt
                    && record.fence == claim.fence
                    && record.state == super::EffectState::Reconciling
            })
            .and_then(|record| record.authority_admission.as_ref())
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "authority_admission",
                    "has no retained executable evidence for this reconciliation",
                )
            })?;
        let original = EffectClaim {
            kind: claim.kind,
            mode: EffectClaimMode::Execute,
            intent_key: claim.intent_key.clone(),
            attempt: claim.attempt,
            reconciliation_attempt: 0,
            fence: admission.fence,
        };
        admission.validate_effect_for(self, &original)?;
        Ok(admission)
    }

    /// Revalidate the exact durable lease immediately before a provider write.
    ///
    /// Admission alone proves that a write may have happened. It does not let
    /// a stalled worker resume writing after its lease has expired.
    pub fn effect_authority_admission_at(
        &self,
        claim: &EffectClaim,
        now: &str,
    ) -> Result<&AuthorityAdmissionSnapshot, TriggerValidationError> {
        let admission = self.effect_authority_admission(claim)?;
        let lease = self.lease.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "worker_lease",
                "is missing at the effect provider-write boundary",
            )
        })?;
        if lease.fencing_token != claim.fence {
            return Err(TriggerValidationError::field(
                "worker_lease",
                "does not own this admitted effect fence",
            ));
        }
        validate_admission_lease(self, lease, now)?;
        Ok(admission)
    }

    pub fn record_capability_authority_admission(
        &mut self,
        lease: &WorkerLease,
        admission: CapabilityAuthorityAdmission,
        now: impl Into<String>,
    ) -> Result<(), TriggerValidationError> {
        let now = now.into();
        validate_admission_lease(self, lease, &now)?;
        admission.validate_for(self, lease)?;
        if let Some(current) = self.capability_authority_admission.as_ref() {
            let now_epoch = crate::seconds_from_rfc3339(&now).ok_or_else(|| {
                TriggerValidationError::field(
                    "capability_authority_admission.observed_at",
                    "is not a CMA timestamp",
                )
            })?;
            if current.target_policy_expires_at_epoch_seconds > now_epoch
                || current.snapshot.fence == lease.fencing_token
                || admission.target_policy_generation <= current.target_policy_generation
            {
                return Err(TriggerValidationError::field(
                    "capability_authority_admission",
                    "has an unsettled policy-install boundary",
                ));
            }
        }
        self.capability_authority_admission = Some(admission);
        self.updated_at = now;
        Ok(())
    }

    pub fn capability_authority_admission_for(
        &self,
        lease: &WorkerLease,
    ) -> Result<&CapabilityAuthorityAdmission, TriggerValidationError> {
        let admission = self
            .capability_authority_admission
            .as_ref()
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "capability_authority_admission",
                    "was not durably committed before policy installation",
                )
            })?;
        admission.validate_for(self, lease)?;
        Ok(admission)
    }

    /// Revalidate a reserved capability generation at its policy-install
    /// boundary. An expired worker lease cannot sign or install the generation.
    pub fn capability_authority_admission_for_at(
        &self,
        lease: &WorkerLease,
        now: &str,
    ) -> Result<&CapabilityAuthorityAdmission, TriggerValidationError> {
        let admission = self.capability_authority_admission_for(lease)?;
        validate_admission_lease(self, lease, now)?;
        Ok(admission)
    }
}

fn validate_admission_lease(
    launch: &Launch,
    lease: &WorkerLease,
    now: &str,
) -> Result<(), TriggerValidationError> {
    let expires = crate::seconds_from_rfc3339(&lease.expires_at).ok_or_else(|| {
        TriggerValidationError::field("worker_lease.expires_at", "is not a CMA timestamp")
    })?;
    let now = crate::seconds_from_rfc3339(now).ok_or_else(|| {
        TriggerValidationError::field("authority_admission.observed_at", "is not a CMA timestamp")
    })?;
    if launch.lease.as_ref() != Some(lease)
        || launch.fencing_counter != lease.fencing_token
        || expires <= now
    {
        return Err(TriggerValidationError::field(
            "worker_lease",
            "does not own this authority-admission mutation",
        ));
    }
    Ok(())
}
