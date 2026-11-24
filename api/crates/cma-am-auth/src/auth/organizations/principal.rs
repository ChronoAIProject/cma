//! Preserve the selected failure before projecting the public membership error.
use layer2_core::resource_access::{
    CallerContext, CredentialContext, MembershipError, ResourceActor,
};
use layer2_core::Caller;

use super::{
    principal_diagnostics::{self, Operation, Phase},
    transport,
};
use crate::auth::{
    bearer, bearer_authorization, permissions::PermissionEvidence, persistence::SecretDigest,
    verification_error::VerificationFailure, AuthMode, AuthState,
};

impl AuthState {
    pub(super) async fn organization_principal(
        &self,
        caller: &Caller,
        credential: &str,
        operation: Operation,
    ) -> Result<CallerContext, MembershipError> {
        let failure = |phase, error: MembershipError| {
            principal_diagnostics::record(operation, phase, &error, None);
            error
        };
        let verification_failure = |phase, error: VerificationFailure| {
            principal_diagnostics::record(
                operation,
                phase,
                &MembershipError::Unavailable,
                Some(&error),
            );
            MembershipError::Unavailable
        };
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return Err(failure(Phase::Configuration, MembershipError::Unavailable));
        };
        if !bearer::permits_personal_authority(credential) {
            return Err(failure(Phase::Restriction, MembershipError::NotMember));
        }
        let metadata = self
            .verification_metadata()
            .await
            .map_err(|error| verification_failure(Phase::Metadata, error.into()))?;
        // The detail is carried by the shared result, but this diagnostic belongs
        // to the failed caller. A leader's task-local context cannot serve waiters.
        let verified = bearer_authorization::verify_subject_detailed(self, &metadata, credential)
            .await
            .map_err(|error| verification_failure(Phase::Verification, error))?;
        let PermissionEvidence::Active(permissions) = &verified.permissions else {
            return Err(failure(Phase::Permissions, MembershipError::Unavailable));
        };
        let subject = &verified.subject;
        if subject != &caller.subject {
            return Err(failure(Phase::Subject, MembershipError::NotMember));
        }
        let context = CredentialContext {
            issuer: base_url.trim_end_matches('/').into(),
            subject: subject.clone(),
            credential_id: transport::digest_id(SecretDigest::identify(
                "organization-caller",
                credential,
            )),
        };
        context
            .validate()
            .map_err(|error| failure(Phase::Context, error))?;
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
