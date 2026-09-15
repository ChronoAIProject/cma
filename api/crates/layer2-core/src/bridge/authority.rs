use std::time::SystemTime;

use crate::resource_access::{
    CallerContext, MembershipError, OrganizationAuthority, OrganizationRole, ResourceActor,
    ResourceOwner,
};
use crate::Capability;

use super::BridgeError;

pub(super) struct BridgeAuthority {
    pub can_manage: bool,
    pub deadline: Option<tokio::time::Instant>,
}

impl BridgeAuthority {
    pub fn require(&self, management: bool) -> Result<(), BridgeError> {
        if self
            .deadline
            .is_some_and(|deadline| tokio::time::Instant::now() >= deadline)
        {
            return Err(BridgeError::AuthorityUnavailable);
        }
        if management && !self.can_manage {
            return Err(BridgeError::Forbidden);
        }
        Ok(())
    }
}

pub(super) async fn authorize(
    provider: &dyn OrganizationAuthority,
    context: &CallerContext,
    credential: Option<&str>,
    owner: &ResourceOwner,
    management: bool,
) -> Result<BridgeAuthority, BridgeError> {
    if !context.validate_principal()
        || !matches!(
            &context.actor,
            ResourceActor::Human { .. }
                | ResourceActor::Api {
                    delegation_id: None,
                    ..
                }
        )
        || !context
            .caller
            .authorization
            .allows(Capability::UseOwnSandboxes)
    {
        return Err(BridgeError::NotFound);
    }
    let result = match owner {
        ResourceOwner::Person { subject } => {
            if subject != &context.caller.subject {
                return Err(BridgeError::NotFound);
            }
            BridgeAuthority {
                can_manage: true,
                deadline: None,
            }
        }
        ResourceOwner::Organization { subject } => {
            let started_at = SystemTime::now();
            let credential = credential.ok_or(BridgeError::AuthorityUnavailable)?;
            let fresh = provider
                .organization_context(&context.caller, credential, subject)
                .await
                .map_err(|error| match error {
                    MembershipError::NotMember => BridgeError::NotFound,
                    _ => BridgeError::AuthorityUnavailable,
                })?;
            if !fresh.validate_principal()
                || fresh.credential != context.credential
                || fresh.caller.subject != context.caller.subject
                || fresh.memberships.len() != 1
                || !fresh
                    .caller
                    .authorization
                    .allows(Capability::UseOwnSandboxes)
            {
                return Err(BridgeError::AuthorityUnavailable);
            }
            let membership = &fresh.memberships[0];
            if membership.organization != *subject
                || !membership.is_current(&context.credential, SystemTime::now())
                || (management && membership.observed_at < started_at)
            {
                return Err(BridgeError::AuthorityUnavailable);
            }
            BridgeAuthority {
                can_manage: membership.role == OrganizationRole::Admin,
                deadline: Some(membership.monotonic_deadline()),
            }
        }
    };
    result.require(management)?;
    Ok(result)
}
