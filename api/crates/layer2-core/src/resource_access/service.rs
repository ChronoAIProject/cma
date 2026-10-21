//! Authenticated resource policy boundary. Public metadata and historical
//! receipts cannot provide the actor, owner or live organization evidence.

use std::{collections::BTreeSet, sync::Arc, time::SystemTime};

use crate::{Authorization, SubjectId};

use super::*;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceAccessError {
    #[error(transparent)]
    Access(#[from] AccessError),
    #[error(transparent)]
    Store(#[from] ResourceAccessStoreError),
    #[error(transparent)]
    Membership(#[from] MembershipError),
}

/// This result is internal evidence, not a public resource projection. A narrow
/// Agent decision authorizes only that Agent; parent metadata needs its own check.
pub struct AuthorizedResource {
    pub snapshot: ResourceAccessSnapshot,
    pub decision: AccessDecision,
    pub(crate) binding: Option<crate::sandbox::binding::BindingConversationAuthority>,
}

#[derive(Clone)]
pub struct ResourceAccessService {
    pub(super) store: Arc<dyn ResourceShareStore>,
    pub(super) bindings: Option<Arc<crate::sandbox::binding::BindingAccessService>>,
    pub(super) organizations: Arc<dyn OrganizationAuthority>,
    pub(super) directory: Option<Arc<crate::UserDirectory>>,
}

impl ResourceAccessService {
    pub fn new(
        store: Arc<dyn ResourceShareStore>,
        organizations: Arc<dyn OrganizationAuthority>,
    ) -> Self {
        Self {
            store,
            organizations,
            directory: None,
            bindings: None,
        }
    }

    /// Read current stores and obtain new evidence for all relevant organizations.
    /// Call again before each deferred effect and before a stream's deadline.
    pub async fn authorize(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        target: &ResourceTarget,
        capability: ResourceCapability,
        purpose: AccessPurpose,
    ) -> Result<AuthorizedResource, ResourceAccessError> {
        if !context.validate_principal() {
            return Err(AccessError::NotFound.into());
        }
        if context.credential_source == CredentialSource::Binding {
            return self.authorize_binding(context, target, capability).await;
        }
        let snapshot = self.store.resource_access_snapshot(target).await?;
        let scope = snapshot.scope(target)?;
        let mut current = context.clone();
        // Admission/stream callers cannot smuggle old evidence into a new check.
        current.memberships.clear();
        let independent =
            AccessDecision::evaluate(&current, &scope, &[], purpose, SystemTime::now())?;
        if independent.allows(capability)
            && (matches!(&scope.owner, ResourceOwner::Person { subject } if subject == &context.caller.subject)
                || !context
                    .caller
                    .authorization
                    .allows(crate::Capability::UseOwnSandboxes))
        {
            return Ok(AuthorizedResource {
                snapshot,
                decision: independent,
                binding: None,
            });
        }
        let organizations: BTreeSet<_> = snapshot
            .shares
            .iter()
            .filter(|share| {
                share
                    .link
                    .as_ref()
                    .is_none_or(|link| link.accepted_subjects.contains(&context.caller.subject))
            })
            .filter_map(|share| {
                share
                    .organization_recipient()
                    .map(|id| id.as_str().to_owned())
            })
            .chain(match &scope.owner {
                ResourceOwner::Organization { subject } => Some(subject.as_str().to_owned()),
                ResourceOwner::Person { .. } => None,
            })
            .collect();
        let mut unavailable = false;
        // Bound the entire lookup set, including a resource with many grants.
        // Successfully obtained independent grants survive another org's outage.
        let lookup = async {
            for organization in organizations {
                let Some(credential) = credential else {
                    unavailable = true;
                    break;
                };
                match self
                    .organizations
                    .organization_context(
                        &context.caller,
                        credential,
                        &SubjectId::new(organization.clone()),
                    )
                    .await
                {
                    Ok(evidence) => {
                        if !evidence.validate_principal()
                            || evidence.credential != context.credential
                            || evidence.caller.subject != context.caller.subject
                            || evidence.memberships.len() != 1
                            || evidence.memberships[0].organization.as_str() != organization
                        {
                            unavailable = true;
                            continue;
                        }
                        current.caller.authorization =
                            Authorization::new(current.caller.authorization.iter().filter(
                                |capability| evidence.caller.authorization.allows(*capability),
                            ));
                        current.memberships.extend(evidence.memberships);
                    }
                    Err(MembershipError::NotMember) => {}
                    Err(_) => unavailable = true,
                }
            }
        };
        if tokio::time::timeout(std::time::Duration::from_secs(15), lookup)
            .await
            .is_err()
        {
            unavailable = true;
        }
        // Membership IO must not hold a stale grant or retain an old root identity.
        let refreshed = self.store.resource_access_snapshot(target).await?;
        if snapshot.fence() != refreshed.fence() {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        let decision = AccessDecision::evaluate(
            &current,
            &refreshed.scope(target)?,
            &refreshed.shares,
            purpose,
            SystemTime::now(),
        )?;
        if let Err(error) = decision.require(capability) {
            if unavailable {
                return Err(MembershipError::Unavailable.into());
            }
            return Err(error.into());
        }
        Ok(AuthorizedResource {
            snapshot: refreshed,
            binding: None,
            decision,
        })
    }

    pub(super) async fn personal_owner(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        target: &ResourceTarget,
    ) -> Result<ResourceAccessSnapshot, ResourceAccessError> {
        if !context.validate_principal() || context.actor.ordinary_subject().is_none() {
            return Err(AccessError::NotFound.into());
        }
        let snapshot = self
            .authorize(
                context,
                credential,
                target,
                ResourceCapability::ManageSharing,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await?
            .snapshot;
        if !matches!(snapshot.scope(target)?.owner, ResourceOwner::Person { subject }
            if subject == context.caller.subject)
        {
            return Err(AccessError::Forbidden.into());
        }
        Ok(snapshot)
    }
}
