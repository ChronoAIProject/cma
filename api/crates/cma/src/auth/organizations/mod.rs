//! Current NyxID membership using exactly the invoking credential. No completed
//! observation is cached or borrowed from another login. The returned context
//! contains no secret and is bounded from the beginning of provider verification.

use std::time::SystemTime;

use layer2_core::resource_access::{
    CallerContext, MembershipError, OrganizationAuthority, OrganizationChoice,
    OrganizationMembership, OrganizationRole, MAX_MEMBERSHIP_AGE,
};
use layer2_core::{Caller, SubjectId};
use serde::Deserialize;

use super::AuthState;

mod principal;
mod principal_diagnostics;
mod request_context;
mod transport;

use principal_diagnostics::Operation;

#[derive(Deserialize)]
struct ProviderOrganization {
    id: String,
    your_role: OrganizationRole,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Deserialize)]
struct ProviderInventory {
    orgs: Vec<ProviderOrganization>,
}

#[async_trait::async_trait]
impl OrganizationAuthority for AuthState {
    /// Fresh evidence for admission, mutations, and deferred effects. The
    /// supplied credential must be the actual caller's selected authority.
    /// This never looks up an owner's ambient stored credential.
    async fn organization_context(
        &self,
        caller: &Caller,
        credential: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        tokio::time::timeout(transport::LOOKUP_TIMEOUT, async {
            let monotonic_start = tokio::time::Instant::now();
            let observed_at = SystemTime::now();
            let mut context = self
                .organization_principal(caller, credential, Operation::Membership)
                .await?;
            let endpoint = self.organization_endpoint(Some(organization))?;
            let organization_result: ProviderOrganization =
                self.organization_json(endpoint, credential).await?;
            // NyxID accepts slugs on the route. Ownership requires the exact
            // canonical org ID, not a slug that happened to resolve today.
            if organization_result.id != organization.as_str() {
                return Err(MembershipError::InvalidEvidence);
            }
            let mut evidence = OrganizationMembership::new(
                context.credential.clone(),
                organization.clone(),
                organization_result.your_role,
                observed_at,
                observed_at + MAX_MEMBERSHIP_AGE,
            )?;
            evidence.bound_monotonic_deadline(monotonic_start + MAX_MEMBERSHIP_AGE);
            if !evidence.is_current(&context.credential, SystemTime::now()) {
                return Err(MembershipError::Unavailable);
            }
            context.memberships.push(evidence);
            Ok(context)
        })
        .await
        .map_err(|_| MembershipError::Unavailable)?
    }

    /// Choices are presentation data, never reusable resource authorization.
    /// Stable ordering permits a CMA inventory projection to paginate by ID.
    async fn organization_choices(
        &self,
        caller: &Caller,
        credential: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        tokio::time::timeout(transport::LOOKUP_TIMEOUT, async {
            self.organization_principal(caller, credential, Operation::Choices)
                .await?;
            let inventory: ProviderInventory = self
                .organization_json(self.organization_endpoint(None)?, credential)
                .await?;
            let mut choices: Vec<_> = inventory
                .orgs
                .into_iter()
                .map(|org| OrganizationChoice {
                    id: SubjectId::new(org.id),
                    role: org.your_role,
                    display_name: org.display_name,
                })
                .collect();
            choices.sort_by(|left, right| left.id.as_str().cmp(right.id.as_str()));
            if choices
                .iter()
                .any(|choice| !transport::canonical_segment(choice.id.as_str()))
                || choices.windows(2).any(|pair| pair[0].id == pair[1].id)
            {
                return Err(MembershipError::InvalidEvidence);
            }
            Ok(choices)
        })
        .await
        .map_err(|_| MembershipError::Unavailable)?
    }
}

#[cfg(test)]
mod tests;
