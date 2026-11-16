//! Current NyxID membership using exactly the invoking credential. No completed
//! observation is cached or borrowed from another login. The returned context
//! contains no secret and is bounded from the beginning of provider verification.

use std::time::{Instant, SystemTime};

use layer2_core::resource_access::{
    CallerContext, CredentialContext, MembershipError, OrganizationAuthority, OrganizationChoice,
    OrganizationMembership, OrganizationRole, ResourceActor, MAX_MEMBERSHIP_AGE,
};
use layer2_core::{Caller, SubjectId};
use serde::Deserialize;

use super::{
    bearer_authorization, permissions::PermissionEvidence, persistence::SecretDigest, AuthMode,
    AuthState,
};

mod diagnostics;
mod request_context;
mod retry;
mod transport;

use diagnostics::{Operation, Reason, Stage};

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
        let started = Instant::now();
        let record = |stage, reason, error: &MembershipError| {
            diagnostics::record(Operation::Membership, stage, reason, None, started, error);
        };
        tokio::time::timeout(transport::LOOKUP_TIMEOUT, async {
            let monotonic_start = tokio::time::Instant::now();
            let observed_at = SystemTime::now();
            let mut context = self
                .organization_principal(caller, credential)
                .await
                .inspect_err(|error| record(Stage::Principal, Reason::Unavailable, error))?;
            let endpoint = self
                .organization_endpoint(Some(organization))
                .inspect_err(|error| record(Stage::Endpoint, Reason::Unavailable, error))?;
            let organization_result: ProviderOrganization = self
                .organization_json(
                    endpoint,
                    credential,
                    Operation::Membership,
                    monotonic_start + transport::LOOKUP_TIMEOUT,
                )
                .await?;
            // NyxID accepts slugs on the route. Ownership requires the exact
            // canonical org ID, not a slug that happened to resolve today.
            if organization_result.id != organization.as_str() {
                record(
                    Stage::Evidence,
                    Reason::InvalidEvidence,
                    &MembershipError::InvalidEvidence,
                );
                return Err(MembershipError::InvalidEvidence);
            }
            let mut evidence = OrganizationMembership::new(
                context.credential.clone(),
                organization.clone(),
                organization_result.your_role,
                observed_at,
                observed_at + MAX_MEMBERSHIP_AGE,
            )
            .inspect_err(|error| record(Stage::Evidence, Reason::InvalidEvidence, error))?;
            evidence.bound_monotonic_deadline(monotonic_start + MAX_MEMBERSHIP_AGE);
            if !evidence.is_current(&context.credential, SystemTime::now()) {
                record(
                    Stage::Evidence,
                    Reason::Unavailable,
                    &MembershipError::Unavailable,
                );
                return Err(MembershipError::Unavailable);
            }
            context.memberships.push(evidence);
            Ok(context)
        })
        .await
        .map_err(|_| {
            record(
                Stage::Deadline,
                Reason::Timeout,
                &MembershipError::Unavailable,
            );
            MembershipError::Unavailable
        })?
    }

    /// Choices are presentation data, never reusable resource authorization.
    /// Stable ordering permits a CMA inventory projection to paginate by ID.
    async fn organization_choices(
        &self,
        caller: &Caller,
        credential: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        let started = Instant::now();
        let record = |stage, reason, error: &MembershipError| {
            diagnostics::record(Operation::Choices, stage, reason, None, started, error);
        };
        let deadline = tokio::time::Instant::now() + transport::LOOKUP_TIMEOUT;
        tokio::time::timeout_at(deadline, async {
            self.organization_principal(caller, credential)
                .await
                .inspect_err(|error| record(Stage::Principal, Reason::Unavailable, error))?;
            let endpoint = self
                .organization_endpoint(None)
                .inspect_err(|error| record(Stage::Endpoint, Reason::Unavailable, error))?;
            let inventory: ProviderInventory = self
                .organization_json(endpoint, credential, Operation::Choices, deadline)
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
                record(
                    Stage::Evidence,
                    Reason::InvalidEvidence,
                    &MembershipError::InvalidEvidence,
                );
                return Err(MembershipError::InvalidEvidence);
            }
            Ok(choices)
        })
        .await
        .map_err(|_| {
            record(
                Stage::Deadline,
                Reason::Timeout,
                &MembershipError::Unavailable,
            );
            MembershipError::Unavailable
        })?
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
        // Share only unfinished verification of this exact credential. Every
        // caller still obtains fresh organization evidence within its deadline.
        let verified = bearer_authorization::verify_subject(self, &metadata, credential)
            .await
            .map_err(|_| MembershipError::Unavailable)?;
        let PermissionEvidence::Active(permissions) = &verified.permissions else {
            return Err(MembershipError::Unavailable);
        };
        let subject = &verified.subject;
        if subject != &caller.subject {
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
                crate::permissions::capabilities_from_permissions(permissions.iter().cloned()),
            ),
            credential: context,
            // Userinfo does not prove a human user_type. Preserve the verified
            // API subject without inventing a human or organization identity.
            actor: ResourceActor::Api {
                subject: subject.clone(),
                delegation_id: None,
            },
            memberships: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests;
