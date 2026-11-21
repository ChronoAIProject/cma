//! Historical decision evidence. Deserializing this data never creates live
//! membership, a resource decision, credential custody or an execution proof.

use serde::{Deserialize, Serialize};

use super::*;
use crate::{SandboxId, SubjectId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAuthorizationAudit {
    pub target: ResourceTarget,
    pub sandbox_id: SandboxId,
    pub sandbox_created_at: String,
    pub owner: ResourceOwner,
    pub actor: ResourceActor,
    /// Zero records the legacy personal policy with no sidecar.
    pub policy_revision: u64,
    pub capability: ResourceCapability,
    pub captured_at: String,
    /// Each observation identifies exactly which capabilities it supplied.
    /// Overlapping access remains explicit, including independent operator access.
    pub observations: Vec<ResourceAuthorityObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceAuthorityObservation {
    pub capabilities: Vec<ResourceCapability>,
    pub basis: ResourceAuthorizationBasis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceAuthorizationBasis {
    PersonalOwner,
    GlobalOperator,
    ExternalBinding {
        binding_id: String,
        generation: u64,
    },
    ShareRecipient {
        subject: SubjectId,
        share: ResourceGrantObservation,
    },
    Organization {
        issuer: String,
        organization: SubjectId,
        role: OrganizationRole,
        observed_at: String,
        expires_at: String,
        /// None denotes the owning organization. A share records its exact
        /// revision and mode, independently of credential custody revisions.
        share: Option<ResourceGrantObservation>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceGrantObservation {
    pub id: String,
    pub revision: u64,
    pub mode: ShareMode,
    pub selector: ResourceSelector,
}

impl ResourceAuthorizationAudit {
    /// Structural historical validation only. Time passing or membership loss
    /// must not invalidate accepted audit evidence or turn it into permission.
    pub fn validate(&self) -> Result<(), ResourceAccessStoreError> {
        let invalid = ResourceAccessStoreError::Corrupt;
        if self.sandbox_id.as_str().is_empty()
            || crate::seconds_from_rfc3339(&self.sandbox_created_at).is_none()
            || crate::seconds_from_rfc3339(&self.captured_at).is_none()
            || self.policy_revision > i64::MAX as u64
            || self.observations.is_empty()
            || self.observations.len() > MAX_ACTIVE_RESOURCE_SHARES + 2
            || !self
                .observations
                .iter()
                .any(|observation| observation.capabilities.contains(&self.capability))
        {
            return Err(invalid);
        }
        for observation in &self.observations {
            if matches!(&observation.basis, ResourceAuthorizationBasis::ExternalBinding { binding_id, generation }
                if binding_id.trim().is_empty() || *generation == 0 || *generation > i64::MAX as u64)
            {
                return Err(invalid);
            }
            if observation.capabilities.is_empty() {
                return Err(invalid);
            }
            if matches!(&observation.basis, ResourceAuthorizationBasis::ShareRecipient { subject, share }
                if subject.as_str().trim().is_empty() || share.id.trim().is_empty()
                    || share.revision == 0 || share.revision > self.policy_revision)
            {
                return Err(invalid);
            }
            if let ResourceAuthorizationBasis::Organization {
                issuer,
                organization,
                observed_at,
                expires_at,
                share,
                ..
            } = &observation.basis
            {
                if issuer.trim().is_empty()
                    || organization.as_str().trim().is_empty()
                    || !crate::seconds_from_rfc3339(observed_at)
                        .zip(crate::seconds_from_rfc3339(expires_at))
                        .is_some_and(|(start, end)| {
                            end >= start && end - start <= MAX_MEMBERSHIP_AGE.as_secs()
                        })
                    || share.as_ref().is_some_and(|share| {
                        share.id.trim().is_empty()
                            || share.revision == 0
                            || share.revision > self.policy_revision
                    })
                {
                    return Err(invalid);
                }
            }
        }
        Ok(())
    }
}

impl AuthorizedResource {
    pub fn authorization_audit(
        &self,
        actor: &ResourceActor,
        target: &ResourceTarget,
        capability: ResourceCapability,
    ) -> Result<ResourceAuthorizationAudit, ResourceAccessError> {
        self.decision.require(capability)?;
        let scope = self.snapshot.scope(target)?;
        Ok(ResourceAuthorizationAudit {
            target: target.clone(),
            sandbox_id: scope.sandbox_id,
            sandbox_created_at: self.snapshot.lineage.sandbox.created_at.clone(),
            owner: scope.owner,
            actor: actor.clone(),
            policy_revision: self.snapshot.fence().policy_revision,
            capability,
            captured_at: crate::now_rfc3339(),
            observations: self.decision.audit_observations().to_vec(),
        })
    }
}
