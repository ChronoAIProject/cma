use serde::{Deserialize, Serialize};

use crate::{AgentId, SandboxId, SubjectId};

use super::{ResourceActor, ResourceOwner, ResourceScope};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceSelector {
    Agent { agent_id: AgentId },
    Sandbox { sandbox_id: SandboxId },
}

impl ResourceSelector {
    pub fn covers(&self, scope: &ResourceScope) -> bool {
        match self {
            Self::Agent { agent_id } => scope.agent_id.as_ref() == Some(agent_id),
            Self::Sandbox { sandbox_id } => &scope.sandbox_id == sandbox_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareMode {
    /// Recipient can inspect the shared Agent/resource.
    View,
    /// Recipient can interact with the Agent and mutate its settings/files,
    /// subject to the same isolation checks as the legacy organization role grant.
    ViewWrite,
    /// Legacy organization role based grant. Kept for backwards compatibility.
    OrgRoles,
}

/// Stored separately from existing resource aggregates. A share is an access
/// grant; it cannot transfer ownership, rewrite origin, or change profile pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceShare {
    pub id: String,
    pub owner: ResourceOwner,
    pub selector: ResourceSelector,
    pub target_org_id: SubjectId,
    pub mode: ShareMode,
    pub issuer_actor: ResourceActor,
    pub created_at: String,
    pub revoked_at: Option<String>,
    pub revision: u64,
}

impl ResourceShare {
    /// Run against current server evidence before creating/updating a grant.
    /// The policy repeats this condition on interaction after admission.
    pub fn require_safe_interaction(
        &self,
        scope: &ResourceScope,
        current_shares: &[Self],
    ) -> Result<(), super::AccessError> {
        if !self.validate() || self.owner != scope.owner || !self.selector.covers(scope) {
            return Err(super::AccessError::NotFound);
        }
        if self.mode == ShareMode::View
            || matches!(self.selector, ResourceSelector::Sandbox { .. })
            || scope.sharing_isolation == SharingIsolation::IndependentlyIsolated
            || current_shares.iter().any(|share| {
                share.validate()
                    && share.owner == self.owner
                    && share.target_org_id == self.target_org_id
                    && share.mode == ShareMode::OrgRoles
                    && share.revoked_at.is_none()
                    && matches!(&share.selector, ResourceSelector::Sandbox { sandbox_id }
                        if sandbox_id == &scope.sandbox_id)
            })
        {
            Ok(())
        } else {
            Err(super::AccessError::UnsafeInteractiveShare)
        }
    }

    pub fn validate(&self) -> bool {
        let ResourceOwner::Person { subject } = &self.owner else {
            return false;
        };
        !self.id.trim().is_empty()
            && !subject.as_str().trim().is_empty()
            && self.issuer_actor.ordinary_subject() == Some(subject)
            && !self.target_org_id.as_str().trim().is_empty()
            && self.revision > 0
            && self.revision <= i64::MAX as u64
            && crate::seconds_from_rfc3339(&self.created_at).is_some()
            && self.revoked_at.as_ref().is_none_or(|time| {
                crate::seconds_from_rfc3339(time).is_some_and(|revoked| {
                    Some(revoked) >= crate::seconds_from_rfc3339(&self.created_at)
                })
            })
            && match &self.selector {
                ResourceSelector::Agent { agent_id } => !agent_id.as_str().trim().is_empty(),
                ResourceSelector::Sandbox { sandbox_id } => !sandbox_id.as_str().trim().is_empty(),
            }
            && if let Some(subject) = self.target_org_id.as_str().strip_prefix("user:") {
                !subject.trim().is_empty() && !subject.bytes().any(|byte| byte.is_ascii_control())
            } else {
                self.target_org_id.as_str() == "public"
                    || !self
                        .target_org_id
                        .as_str()
                        .bytes()
                        .any(|byte| byte.is_ascii_control())
            }
    }
}

/// Supplied from current server-side isolation evidence, never a share DTO or
/// a count of Agents. Worktrees and separate homes are not isolation evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharingIsolation {
    Unproved,
    IndependentlyIsolated,
}
