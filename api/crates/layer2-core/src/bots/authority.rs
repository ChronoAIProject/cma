//! Verified creation authority is distinct from the owner of the new resources.
use super::BotError;
use crate::resource_access::{
    CallerContext, CredentialSource, OrganizationRole, ResourceActor, ResourceOwner,
};
use crate::{Capability, SubjectId};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Nonsecret references to an explicitly approved, renewable delegation.
/// The authentication adapter verifies both references before constructing authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotDelegation {
    pub authorization_grant_id: String,
    pub credential_incarnation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotOwnership {
    pub owner: ResourceOwner,
    pub created_by: ResourceActor,
    pub credential_subject: SubjectId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_incarnation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_grant_id: Option<String>,
}

/// Constructed by trusted authentication after validating the invoking grant.
/// It cannot be deserialized from requests or from persisted ownership metadata.
#[derive(Debug, Clone)]
pub struct BotOwnerAuthority {
    context: CallerContext,
    ownership: BotOwnership,
}

impl BotOwnerAuthority {
    pub fn new(
        context: CallerContext,
        owner: ResourceOwner,
        delegation: Option<BotDelegation>,
    ) -> Result<Self, BotError> {
        let ownership = BotOwnership {
            owner,
            created_by: context.actor.clone(),
            credential_subject: context.caller.subject.clone(),
            credential_incarnation: delegation
                .as_ref()
                .map(|value| value.credential_incarnation.clone()),
            authorization_grant_id: delegation.map(|value| value.authorization_grant_id),
        };
        let value = Self { context, ownership };
        value.validate()?;
        Ok(value)
    }

    pub fn owner(&self) -> &ResourceOwner {
        &self.ownership.owner
    }
    pub fn owner_subject(&self) -> &SubjectId {
        owner_subject(self.owner())
    }
    pub fn actor(&self) -> &ResourceActor {
        &self.ownership.created_by
    }
    pub fn credential_subject(&self) -> &SubjectId {
        &self.ownership.credential_subject
    }
    pub fn ownership(&self) -> &BotOwnership {
        &self.ownership
    }

    pub fn require(&self, capability: Capability) -> Result<(), BotError> {
        self.validate()?;
        self.context
            .caller
            .require(capability)
            .map_err(|_| BotError::Forbidden)
    }

    fn validate(&self) -> Result<(), BotError> {
        self.ownership.validate()?;
        let context = &self.context;
        if context.credential.validate().is_err()
            || context.credential.subject != context.caller.subject
            || matches!(
                context.credential_source,
                CredentialSource::Binding | CredentialSource::RequestBoundRead { .. }
            )
            || matches!(context.credential_source, CredentialSource::Independent { expires_at: Some(expiry) } if expiry <= SystemTime::now())
        {
            return Err(BotError::Forbidden);
        }
        match self.owner() {
            ResourceOwner::Person { subject } if subject == &context.caller.subject => Ok(()),
            ResourceOwner::Organization { subject } => {
                let mut evidence = context
                    .memberships
                    .iter()
                    .filter(|value| &value.organization == subject);
                match (evidence.next(), evidence.next()) {
                    (Some(value), None)
                        if value.role == OrganizationRole::Admin
                            && value.is_current(&context.credential, SystemTime::now()) =>
                    {
                        Ok(())
                    }
                    _ => Err(BotError::Forbidden),
                }
            }
            _ => Err(BotError::Forbidden),
        }
    }
}

impl BotOwnership {
    pub fn validate(&self) -> Result<(), BotError> {
        if owner_subject(&self.owner).as_str().trim().is_empty()
            || self.credential_subject.as_str().trim().is_empty()
        {
            return Err(BotError::Forbidden);
        }
        let actor = match &self.created_by {
            ResourceActor::Human { subject }
                if self.authorization_grant_id.is_none()
                    && self.credential_incarnation.is_none() =>
            {
                subject
            }
            ResourceActor::Api {
                subject,
                delegation_id,
            } if delegation_id == &self.authorization_grant_id => {
                match (&self.authorization_grant_id, &self.credential_incarnation) {
                    (None, None) => {}
                    (Some(grant), Some(incarnation))
                        if !grant.trim().is_empty() && !incarnation.trim().is_empty() => {}
                    _ => return Err(BotError::Forbidden),
                }
                subject
            }
            _ => return Err(BotError::Forbidden),
        };
        if actor != &self.credential_subject {
            return Err(BotError::Forbidden);
        }
        if matches!(&self.owner, ResourceOwner::Person {subject} if subject != actor) {
            return Err(BotError::Forbidden);
        }
        Ok(())
    }
}

pub(crate) fn owner_subject(owner: &ResourceOwner) -> &SubjectId {
    match owner {
        ResourceOwner::Person { subject } | ResourceOwner::Organization { subject } => subject,
    }
}
