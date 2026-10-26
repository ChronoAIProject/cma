use std::collections::BTreeSet;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{AgentId, Caller, Capability, SandboxId, SubjectId};

use super::{
    CallerContext, OrganizationRole, ResourceOwner, ResourceSelector, ResourceShare, ShareMode,
    SharingIsolation,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceCapability {
    ViewDetails,
    ViewHistory,
    /// Owner-approved immutable Agent exports; never a runtime filesystem grant.
    ViewArtifacts,
    ViewFiles,
    Interact,
    ResolveApproval,
    OperateAgent,
    ManageSandbox,
    ManageBridge,
    ManageSharing,
}

#[derive(Debug, Clone, Copy)]
pub enum AccessPurpose {
    Read,
    /// Every mutation/retry must start a new check. A previous admission or
    /// cached read observation is insufficient for a deferred effect.
    Mutation {
        started_at: SystemTime,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceScope {
    pub sandbox_id: SandboxId,
    pub agent_id: Option<AgentId>,
    pub owner: ResourceOwner,
    pub sharing_isolation: SharingIsolation,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccessError {
    #[error("resource not found")]
    NotFound,
    #[error("this visible resource does not permit the requested operation")]
    Forbidden,
    #[error("current organization authorization is required")]
    RevalidationRequired,
    #[error("interactive Agent sharing requires independent isolation or a compatible whole-Sandbox grant")]
    UnsafeInteractiveShare,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessDecision {
    capabilities: BTreeSet<ResourceCapability>,
    independent: BTreeSet<ResourceCapability>,
    deadline: Option<tokio::time::Instant>,
    /// A stream/ticket must revalidate by this deadline. Independent owner or
    /// operator capabilities remain usable through independent authority.
    pub revalidate_at: Option<SystemTime>,
    pub share_revisions: Vec<(String, u64)>,
    observations: Vec<super::ResourceAuthorityObservation>,
}

impl AccessDecision {
    /// Monotonic lifetime for this capability; independent authority has no
    /// organization deadline. A stream must stop or obtain new evidence by it.
    pub fn revalidation_deadline(
        &self,
        capability: ResourceCapability,
    ) -> Option<tokio::time::Instant> {
        if self.independent.contains(&capability) {
            None
        } else {
            self.deadline
        }
    }

    pub fn allows(&self, capability: ResourceCapability) -> bool {
        self.independent.contains(&capability)
            || (self.capabilities.contains(&capability)
                && self
                    .deadline
                    .is_none_or(|deadline| tokio::time::Instant::now() < deadline))
    }

    pub fn require(&self, capability: ResourceCapability) -> Result<(), AccessError> {
        if self.allows(capability) {
            Ok(())
        } else if self.capabilities.contains(&capability) {
            Err(AccessError::RevalidationRequired)
        } else if self.capabilities.is_empty() {
            Err(AccessError::NotFound)
        } else {
            Err(AccessError::Forbidden)
        }
    }

    pub fn capabilities(&self) -> impl Iterator<Item = ResourceCapability> + '_ {
        self.capabilities
            .iter()
            .copied()
            .filter(|capability| self.allows(*capability))
    }

    /// The caller must read current ownership, shares and isolation from the
    /// resource policy store for each decision. Historical audit is not a store.
    pub fn evaluate(
        context: &CallerContext,
        scope: &ResourceScope,
        shares: &[ResourceShare],
        purpose: AccessPurpose,
        now: SystemTime,
    ) -> Result<Self, AccessError> {
        let super::CredentialSource::RequestBoundRead { expires_at } = context.credential_source
        else {
            return Self::evaluate_authority(context, scope, shares, purpose, now);
        };
        if !matches!(purpose, AccessPurpose::Read) || expires_at <= now {
            return Ok(Self::empty());
        }
        let mut result = Self::evaluate_authority(context, scope, shares, purpose, now)?;
        result.capabilities.retain(|capability| {
            matches!(
                capability,
                ResourceCapability::ViewDetails | ResourceCapability::ViewHistory
            )
        });
        result.independent.clear();
        if let Some(deadline) = context.credential_deadline() {
            result.deadline = Some(result.deadline.map_or(deadline, |old| old.min(deadline)));
        }
        result.revalidate_at = Some(
            result
                .revalidate_at
                .map_or(expires_at, |old| old.min(expires_at)),
        );
        Ok(result)
    }

    fn evaluate_authority(
        context: &CallerContext,
        scope: &ResourceScope,
        shares: &[ResourceShare],
        purpose: AccessPurpose,
        now: SystemTime,
    ) -> Result<Self, AccessError> {
        let mut result = Self::empty();
        if !context.validate_principal() {
            return Ok(result);
        }
        let authority = &context.caller.authorization;
        if let ResourceOwner::Person { subject } = &scope.owner {
            if subject == &context.caller.subject && authority.allows(Capability::UseOwnSandboxes) {
                result.grant_owner();
                result.independent = result.capabilities.clone();
                result.observe_independent(super::ResourceAuthorizationBasis::PersonalOwner);
                return Ok(result);
            }
        }
        // Global operator authority remains explicit and grants no sharing,
        // bridge-management, profile-authoring or credential custody privileges.
        if authority.allows(Capability::ReadAllSandboxes) {
            result.grant_view(true);
        }
        if authority.allows(Capability::ControlAnySandbox) {
            result.grant_view(true);
            result.grant_interaction();
            result
                .capabilities
                .insert(ResourceCapability::ManageSandbox);
        }
        result.independent = result.capabilities.clone();
        result.observe_independent(super::ResourceAuthorizationBasis::GlobalOperator);
        if !authority.allows(Capability::UseOwnSandboxes) {
            return Ok(result);
        }
        let mut needs_revalidation = false;
        if let ResourceOwner::Organization { subject } = &scope.owner {
            if let Some(evidence) = match membership(context, subject, purpose, now) {
                Ok(evidence) => evidence,
                Err(_) => {
                    needs_revalidation = true;
                    None
                }
            } {
                let mut granted = Self::empty();
                granted.grant_view(true);
                if evidence.role == OrganizationRole::Admin {
                    granted.grant_owner();
                }
                result.observe_membership(evidence, granted, None);
            }
        }
        for share in shares.iter().filter(|share| {
            share.validate()
                && share.owner == scope.owner
                && share.revoked_at.is_none()
                && share.selector.covers(scope)
        }) {
            if let Some(link) = &share.link {
                if !link.accepted_subjects.contains(&context.caller.subject) {
                    continue;
                }
                let mut granted = Self::empty();
                // Explicit Agent-link consent covers the shared home but never
                // the parent Sandbox's lifecycle or the owner's credentials.
                granted.grant_view(true);
                if share.mode == ShareMode::ViewWrite {
                    granted.grant_interaction();
                }
                if let Some(organization) = link.audience.organization() {
                    let evidence = match membership(context, organization, purpose, now) {
                        Ok(Some(evidence)) => evidence,
                        Ok(None) => continue,
                        Err(_) => {
                            needs_revalidation = true;
                            continue;
                        }
                    };
                    result.observe_membership(evidence, granted, Some(share));
                } else {
                    result.capabilities.extend(&granted.capabilities);
                    result.observe(
                        now + std::time::Duration::from_secs(30),
                        tokio::time::Instant::now() + std::time::Duration::from_secs(30),
                    );
                    result
                        .observations
                        .push(super::ResourceAuthorityObservation {
                            capabilities: granted.capabilities.iter().copied().collect(),
                            basis: super::ResourceAuthorizationBasis::ShareRecipient {
                                subject: context.caller.subject.clone(),
                                share: super::ResourceGrantObservation {
                                    id: share.id.clone(),
                                    revision: share.revision,
                                    mode: share.mode,
                                    selector: share.selector.clone(),
                                },
                            },
                        });
                }
                result
                    .share_revisions
                    .push((share.id.clone(), share.revision));
                continue;
            }
            let evidence = match membership(context, &share.target_org_id, purpose, now) {
                Ok(Some(evidence)) => evidence,
                Ok(None) => continue,
                Err(_) => {
                    needs_revalidation = true;
                    continue;
                }
            };
            let whole_sandbox = matches!(share.selector, ResourceSelector::Sandbox { .. });
            let mut granted = Self::empty();
            granted.grant_view(whole_sandbox);
            if share.mode == ShareMode::OrgRoles && evidence.role == OrganizationRole::Admin {
                // An unsafe narrow grant never gains interaction, even if a
                // historical creation check ran before the Sandbox changed.
                if whole_sandbox
                    || scope.sharing_isolation == SharingIsolation::IndependentlyIsolated
                    || shares.iter().any(|other| {
                        other.validate()
                            && other.owner == scope.owner
                            && other.target_org_id == share.target_org_id
                            && other.mode == ShareMode::OrgRoles
                            && other.revoked_at.is_none()
                            && matches!(&other.selector, ResourceSelector::Sandbox { sandbox_id }
                                if sandbox_id == &scope.sandbox_id)
                    })
                {
                    granted.grant_interaction();
                    if whole_sandbox {
                        granted
                            .capabilities
                            .insert(ResourceCapability::ManageSandbox);
                    }
                }
            }
            result.observe_membership(evidence, granted, Some(share));
            result
                .share_revisions
                .push((share.id.clone(), share.revision));
        }
        if result.capabilities.is_empty() && needs_revalidation {
            return Err(AccessError::RevalidationRequired);
        }
        Ok(result)
    }

    /// Retained owner-only boundaries use the same personal policy while their
    /// organization/share projections are introduced. No subject-kind inference.
    pub fn personal(caller: &Caller, owner: &SubjectId) -> Self {
        let mut result = Self::empty();
        if &caller.subject == owner && caller.authorization.allows(Capability::UseOwnSandboxes) {
            result.grant_owner();
            result.independent = result.capabilities.clone();
            result.observe_independent(super::ResourceAuthorizationBasis::PersonalOwner);
        }
        result
    }

    pub(crate) fn binding(
        proof: &crate::sandbox::binding::BindingConversationAuthority,
        revive_parent: bool,
    ) -> Self {
        let mut result = Self::empty();
        result.capabilities.extend(if revive_parent {
            vec![ResourceCapability::ManageSandbox]
        } else {
            vec![
                ResourceCapability::ViewDetails,
                ResourceCapability::ViewHistory,
                ResourceCapability::Interact,
                ResourceCapability::OperateAgent,
            ]
        });
        result.independent = result.capabilities.clone();
        result.observe_independent(super::ResourceAuthorizationBasis::ExternalBinding {
            binding_id: proof.binding.binding_id.clone(),
            generation: proof.binding.generation,
        });
        result
    }

    fn empty() -> Self {
        Self {
            capabilities: BTreeSet::new(),
            independent: BTreeSet::new(),
            deadline: None,
            revalidate_at: None,
            share_revisions: Vec::new(),
            observations: Vec::new(),
        }
    }

    pub(super) fn audit_observations(&self) -> &[super::ResourceAuthorityObservation] {
        &self.observations
    }

    fn observe_independent(&mut self, basis: super::ResourceAuthorizationBasis) {
        if !self.independent.is_empty() {
            self.observations.push(super::ResourceAuthorityObservation {
                capabilities: self.independent.iter().copied().collect(),
                basis,
            });
        }
    }

    fn observe_membership(
        &mut self,
        evidence: &super::OrganizationMembership,
        granted: Self,
        share: Option<&ResourceShare>,
    ) {
        self.capabilities.extend(&granted.capabilities);
        self.observe(evidence.expires_at, evidence.monotonic_deadline());
        self.observations.push(super::ResourceAuthorityObservation {
            capabilities: granted.capabilities.into_iter().collect(),
            basis: super::ResourceAuthorizationBasis::Organization {
                issuer: evidence.credential.issuer.clone(),
                organization: evidence.organization.clone(),
                role: evidence.role,
                observed_at: crate::rfc3339_at(evidence.observed_at),
                expires_at: crate::rfc3339_at(evidence.expires_at),
                share: share.map(|share| super::ResourceGrantObservation {
                    id: share.id.clone(),
                    revision: share.revision,
                    mode: share.mode,
                    selector: share.selector.clone(),
                }),
            },
        });
    }

    fn observe(&mut self, expires: SystemTime, deadline: tokio::time::Instant) {
        self.revalidate_at = Some(self.revalidate_at.map_or(expires, |old| old.min(expires)));
        self.deadline = Some(self.deadline.map_or(deadline, |old| old.min(deadline)));
    }

    fn grant_view(&mut self, files: bool) {
        self.capabilities.extend([
            ResourceCapability::ViewDetails,
            ResourceCapability::ViewHistory,
            ResourceCapability::ViewArtifacts,
        ]);
        if files {
            self.capabilities.insert(ResourceCapability::ViewFiles);
        }
    }

    fn grant_interaction(&mut self) {
        self.capabilities.extend([
            ResourceCapability::Interact,
            ResourceCapability::ResolveApproval,
            ResourceCapability::OperateAgent,
        ]);
    }

    fn grant_owner(&mut self) {
        self.grant_view(true);
        self.grant_interaction();
        self.capabilities.extend([
            ResourceCapability::ManageSandbox,
            ResourceCapability::ManageBridge,
            ResourceCapability::ManageSharing,
        ]);
    }
}

fn membership<'a>(
    context: &'a CallerContext,
    organization: &SubjectId,
    purpose: AccessPurpose,
    now: SystemTime,
) -> Result<Option<&'a super::OrganizationMembership>, AccessError> {
    let mut matches = context.memberships.iter().filter(|evidence| {
        &evidence.organization == organization && evidence.credential == context.credential
    });
    let Some(evidence) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some()
        || !evidence.is_current(&context.credential, now)
        || matches!(purpose, AccessPurpose::Mutation { started_at }
            if started_at > now || evidence.observed_at < started_at)
    {
        return Err(AccessError::RevalidationRequired);
    }
    Ok(Some(evidence))
}
