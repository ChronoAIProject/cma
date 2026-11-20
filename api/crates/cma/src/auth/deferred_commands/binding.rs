use super::*;
use layer2_core::resource_access::{CallerContext, CredentialSource};
use layer2_core::sandbox::binding::BindingError;

impl AuthState {
    pub(super) async fn binding_admission_authority(
        &self,
        context: &CallerContext,
        credential: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        let request = self
            .binding_request(credential.to_owned())
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?
            .ok_or(CommandAuthorizationError::Revoked)?;
        let principal = request.binding.ok_or(CommandAuthorizationError::Revoked)?;
        if context.credential_source != CredentialSource::Binding
            || *context != crate::auth::binding::context(&principal)
            || owner != principal.resource_owner()
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(principal.command_authority())
    }

    pub(super) async fn retained_binding_context(
        &self,
        envelope: &AgentCommandEnvelope,
    ) -> Result<CallerContext, CommandAuthorizationError> {
        self.require_actor_owner_accounts(&envelope.actor, &envelope.owner)
            .await?;
        envelope
            .validate_integrity()
            .map_err(|_| CommandAuthorizationError::Revoked)?;
        let id = envelope
            .authorization
            .authority_id
            .strip_prefix("binding:")
            .ok_or(CommandAuthorizationError::Revoked)?;
        let generation = envelope
            .authorization
            .grant_revision
            .ok_or(CommandAuthorizationError::Revoked)?;
        let bindings = self
            .bindings
            .as_ref()
            .ok_or(CommandAuthorizationError::Unavailable)?;
        let principal = bindings
            .retained_principal(id, generation)
            .await
            .map_err(binding_error)?;
        let proof = bindings
            .resolve(&principal, envelope.agent_id.as_str())
            .await
            .map_err(binding_error)?;
        proof
            .require_command(envelope)
            .map_err(|_| CommandAuthorizationError::Revoked)?;
        Ok(crate::auth::binding::context(&principal))
    }
}

fn binding_error(error: BindingError) -> CommandAuthorizationError {
    match error {
        BindingError::Unavailable(_) | BindingError::Corrupt(_) | BindingError::Conflict => {
            CommandAuthorizationError::Unavailable
        }
        _ => CommandAuthorizationError::Revoked,
    }
}
