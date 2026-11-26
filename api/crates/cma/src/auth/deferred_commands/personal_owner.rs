//! Recover an owner's exact retained sign-in for an explicitly approved operation.

use layer2_core::resource_access::{CallerContext, CredentialSource};
use layer2_core::SubjectId;

use super::{custody::CommandCustody, *};
use crate::auth::persistence::SecretValue;

impl AuthState {
    /// Return the bearer and its exact retained login revision from the same
    /// refreshed subject state. An unavailable revision leaves the ordinary
    /// credential issuance usable without an independent retirement selector.
    pub(crate) async fn delegated_issuance_credential(
        &self,
        owner: &SubjectId,
    ) -> Result<Option<(SecretValue, Option<AgentAuthorizationRevision>)>, layer2_core::StoreError>
    {
        let Some(mut state) = self.store.subject(owner).await? else {
            return Ok(None);
        };
        if state.subject != *owner {
            return Err(layer2_core::StoreError::Malformed(
                "subject credential crossed its identity boundary".into(),
            ));
        }
        crate::auth::refresh::refresh_if_needed(self, &mut state).await?;
        let revision = self.command_authority_revision(&state).ok();
        Ok(state.tokens.and_then(|tokens| {
            (tokens.expires_at > std::time::SystemTime::now())
                .then_some((tokens.access_token, revision))
        }))
    }

    /// Recover only the personal sign-in incarnation captured at owner approval.
    /// Refresh within that incarnation is allowed; another login, an independent
    /// API credential, or a revoked grant cannot supply replacement authority.
    pub(crate) async fn retained_personal_owner_authority(
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
