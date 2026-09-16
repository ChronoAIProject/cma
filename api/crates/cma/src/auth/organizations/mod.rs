//! Current NyxID membership using exactly the invoking credential. No completed
//! observation is cached or borrowed from another login. The returned context
//! contains no secret and is bounded from the beginning of provider verification.

use std::time::SystemTime;

use layer2_core::resource_access::{
    CallerContext, CredentialContext, MembershipError, OrganizationAuthority, OrganizationChoice,
    OrganizationMembership, OrganizationRole, ResourceActor, MAX_MEMBERSHIP_AGE,
};
use layer2_core::{Caller, SubjectId};
use serde::Deserialize;

use super::{identity, persistence::SecretDigest, AuthMode, AuthState};

mod request_context;
mod transport;

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
            let mut context = self.organization_principal(caller, credential).await?;
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
            self.organization_principal(caller, credential).await?;
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

impl AuthState {
    async fn organization_principal(
        &self,
        caller: &Caller,
        credential: &str,
    ) -> Result<CallerContext, MembershipError> {
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return Err(MembershipError::Unavailable);
        };
        if !super::bearer::permits_personal_authority(credential) {
            return Err(MembershipError::NotMember);
        }
        let metadata = self
            .metadata()
            .await
            .map_err(|_| MembershipError::Unavailable)?;
        let (subject, permissions, _) = identity::resolve_subject(self, &metadata, credential)
            .await
            .map_err(|_| MembershipError::Unavailable)?;
        if subject != caller.subject {
            return Err(MembershipError::NotMember);
        }
        let context = CredentialContext {
            issuer: base_url.trim_end_matches('/').into(),
            subject: subject.clone(),
            credential_id: transport::digest_id(SecretDigest::identify(
                "organization-caller",
                credential,
            )),
        };
        context.validate()?;
        Ok(CallerContext {
            credential_source: layer2_core::resource_access::CredentialSource::Retained,
            caller: Caller::new(
                subject.clone(),
                crate::permissions::capabilities_from_permissions(permissions),
            ),
            credential: context,
            // Userinfo does not prove a human user_type. Preserve the verified
            // API subject without inventing a human or organization identity.
            actor: ResourceActor::Api {
                subject,
                delegation_id: None,
            },
            memberships: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests;
