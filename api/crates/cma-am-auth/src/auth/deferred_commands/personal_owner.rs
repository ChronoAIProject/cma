//! Recover an owner's exact retained sign-in for an explicitly approved operation.

use layer2_core::resource_access::{CallerContext, CredentialSource};
use layer2_core::SubjectId;

use super::{custody::CommandCustody, *};
use crate::auth::persistence::SecretValue;

impl AuthState {
    /// Recover only the personal sign-in incarnation captured at owner approval.
    /// Refresh within that incarnation is allowed; another login, an independent
    /// API credential, or a revoked grant cannot supply replacement authority.
    pub async fn retained_personal_owner_authority(
        &self,
        owner: &SubjectId,
        authorization: &AgentAuthorizationRevision,
    ) -> Result<(CallerContext, SecretValue), CommandAuthorizationError> {
        tokio::time::timeout(VERIFICATION_TIMEOUT, async {
            if !authorization.authority_id.starts_with("personal:") {
                return Err(CommandAuthorizationError::Revoked);
            }
            let custody =
                CommandCustody::Personal(self.retained_actor_state(owner, authorization).await?);
            let mut request = self.verify_command_custody(&custody).await?;
            if request.subject != *owner
                || request.binding.is_some()
                || request.credential_source != CredentialSource::Retained
                || !request.authorization.allows(Capability::UseOwnSandboxes)
            {
                return Err(CommandAuthorizationError::Revoked);
            }
            let mut context = self.resource_request_context(&request);
            context.actor = AgentActor::Human {
                subject: owner.clone(),
            };
            let credential = SecretValue::new(
                request
                    .delegated_credential
                    .take()
                    .ok_or(CommandAuthorizationError::Revoked)?,
            );
            if !context.validate_principal()
                || self.provider_credential_context(owner, credential.expose())
                    != Some(context.credential.clone())
            {
                return Err(CommandAuthorizationError::Revoked);
            }
            self.require_command_custody(&custody).await?;
            Ok((context, credential))
        })
        .await
        .map_err(|_| CommandAuthorizationError::Unavailable)?
    }
}
