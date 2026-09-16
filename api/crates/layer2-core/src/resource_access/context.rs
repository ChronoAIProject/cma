use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::{Caller, SubjectId};

use super::ResourceActor;

/// A read or stream must obtain new evidence at or before this deadline.
pub const MAX_MEMBERSHIP_AGE: Duration = Duration::from_secs(60);

/// A nonsecret identity for one authenticated credential context. It must change
/// on credential replacement and include the configured issuer. No raw token
/// or credential custody is serialized into an Agent, share or command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialContext {
    pub issuer: String,
    pub subject: SubjectId,
    pub credential_id: String,
}

impl CredentialContext {
    pub fn validate(&self) -> Result<(), MembershipError> {
        if self.issuer.trim().is_empty()
            || self.subject.as_str().trim().is_empty()
            || self.credential_id.trim().is_empty()
        {
            return Err(MembershipError::InvalidEvidence);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationRole {
    Admin,
    Member,
    Viewer,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MembershipError {
    #[error("organization membership is not available to this principal")]
    NotMember,
    #[error("organization authorization evidence is invalid")]
    InvalidEvidence,
    #[error(
        "organization authorization is unavailable; retry after the identity provider recovers"
    )]
    Unavailable,
}

/// A live provider observation bound to the exact requesting authority. The
/// start time bounds the observation even if the provider response is delayed.
/// This type deliberately cannot be deserialized from client or audit data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizationMembership {
    pub credential: CredentialContext,
    pub organization: SubjectId,
    pub role: OrganizationRole,
    pub observed_at: SystemTime,
    pub expires_at: SystemTime,
    monotonic_deadline: tokio::time::Instant,
}

impl OrganizationMembership {
    pub fn new(
        credential: CredentialContext,
        organization: SubjectId,
        role: OrganizationRole,
        observed_at: SystemTime,
        credential_expires_at: SystemTime,
    ) -> Result<Self, MembershipError> {
        credential.validate()?;
        if organization.as_str().trim().is_empty() {
            return Err(MembershipError::InvalidEvidence);
        }
        let expires_at = observed_at
            .checked_add(MAX_MEMBERSHIP_AGE)
            .ok_or(MembershipError::InvalidEvidence)?
            .min(credential_expires_at);
        if expires_at <= observed_at {
            return Err(MembershipError::InvalidEvidence);
        }
        Ok(Self {
            credential,
            organization,
            role,
            observed_at,
            expires_at,
            monotonic_deadline: tokio::time::Instant::now()
                + expires_at
                    .duration_since(observed_at)
                    .map_err(|_| MembershipError::InvalidEvidence)?,
        })
    }

    /// Tighten the lifetime to include provider IO. This can never extend it.
    pub fn bound_monotonic_deadline(&mut self, deadline: tokio::time::Instant) {
        self.monotonic_deadline = self.monotonic_deadline.min(deadline);
    }

    pub fn monotonic_deadline(&self) -> tokio::time::Instant {
        self.monotonic_deadline
    }

    pub fn is_current(&self, credential: &CredentialContext, now: SystemTime) -> bool {
        tokio::time::Instant::now() < self.monotonic_deadline
            && &self.credential == credential
            && self.credential.validate().is_ok()
            && !self.organization.as_str().trim().is_empty()
            && self.observed_at <= now
            && now < self.expires_at
            && self
                .expires_at
                .duration_since(self.observed_at)
                .is_ok_and(|age| age <= MAX_MEMBERSHIP_AGE)
    }
}

/// Trusted custody selection; never deserialized from a public request or audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    /// Authority established by an existing retained sign-in or internal adapter.
    Retained,
    /// Exact independently presented credential; cannot borrow sign-in custody.
    Independent { expires_at: Option<SystemTime> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Trusted request context. A failed organization lookup does not remove
/// independent personal access, and custody remains with the actual caller.
pub struct CallerContext {
    pub credential_source: CredentialSource,
    pub caller: Caller,
    pub credential: CredentialContext,
    pub actor: ResourceActor,
    pub memberships: Vec<OrganizationMembership>,
}

impl CallerContext {
    /// The credential may expire before a resource/membership proof does.
    pub fn credential_deadline(&self) -> Option<tokio::time::Instant> {
        let CredentialSource::Independent {
            expires_at: Some(expires_at),
        } = self.credential_source
        else {
            return None;
        };
        let now = tokio::time::Instant::now();
        Some(
            now.checked_add(
                expires_at
                    .duration_since(SystemTime::now())
                    .unwrap_or_default(),
            )
            .unwrap_or(now),
        )
    }

    /// Require actual principal attribution for ordinary CMA requests. A
    /// channel/delegated service will use its separately scoped bridge policy.
    pub fn validate_principal(&self) -> bool {
        self.credential.validate().is_ok()
            && self.caller.subject == self.credential.subject
            && matches!(&self.actor,
                ResourceActor::Human { subject }
                | ResourceActor::Api { subject, delegation_id: None }
                | ResourceActor::Trigger { subject, .. }
                if subject == &self.caller.subject)
    }
}
