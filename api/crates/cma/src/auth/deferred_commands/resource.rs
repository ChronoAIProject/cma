use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::agent_io::AgentSourceKind;

use super::*;
use crate::auth::refresh;

impl AuthState {
    pub(super) async fn retained_command_revival(
        &self,
        access: &ResourceAccessService,
        command: &layer2_core::sandbox::agent_io::AdmittedAgentCommand,
    ) -> Result<
        std::sync::Arc<layer2_core::sandbox::agent_io::CommandRevivalAuthorization>,
        ResourceCommandAuthorizationError,
    > {
        let envelope = &command.receipt.envelope;
        self.require_actor_owner_accounts(&envelope.actor, &envelope.owner)
            .await?;
        if envelope.authorization.authority_id.starts_with("binding:") {
            let context = self.retained_binding_context(envelope).await?;
            return Ok(access
                .authorize_command_revival(&context, "", command)
                .await?);
        }
        let state = self.retained_command_state(envelope).await?;
        let request = self.verify_command_custody(&state).await?;
        let mut context = self.resource_request_context(&request);
        context.actor = envelope.actor.clone();
        let credential = request
            .delegated_credential
            .as_deref()
            .ok_or(CommandAuthorizationError::Revoked)?;
        let proof = access
            .authorize_command_revival(&context, credential, command)
            .await?;
        self.require_command_custody(&state).await?;
        proof.require_current()?;
        Ok(proof)
    }

    /// Recover exactly the sign-in or independent API credential that admitted
    /// the command, independently of resource ownership and sharing policy.
    pub(super) async fn retained_command_state(
        &self,
        envelope: &AgentCommandEnvelope,
    ) -> Result<super::custody::CommandCustody, CommandAuthorizationError> {
        self.require_actor_owner_accounts(&envelope.actor, &envelope.owner)
            .await?;
        envelope
            .validate_integrity()
            .map_err(|_| CommandAuthorizationError::Revoked)?;
        if !matches!(
            envelope.source.kind,
            AgentSourceKind::Api | AgentSourceKind::UserInterface
        ) || ((envelope.authorization.authority_id.starts_with("api:")
            || envelope
                .authorization
                .authority_id
                .starts_with("nyxid-proxy:"))
            && envelope.source.kind != AgentSourceKind::Api)
            || (matches!(
                &envelope.actor,
                AgentActor::Api {
                    delegation_id: Some(_),
                    ..
                }
            ) && envelope.source.kind != AgentSourceKind::Api)
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        self.command_actor_custody(&envelope.actor, &envelope.authorization)
            .await
    }

    /// The exact admitted human/API sign-in is independent of the kind of
    /// resource command. This helper never invents an Agent or owner caller.
    pub(super) async fn retained_actor_state(
        &self,
        subject: &layer2_core::SubjectId,
        authorization: &AgentAuthorizationRevision,
    ) -> Result<SubjectAuthState, CommandAuthorizationError> {
        let mut state = self
            .store
            .subject(subject)
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?
            .ok_or(CommandAuthorizationError::Revoked)?;
        if state.subject != *subject
            || &self.command_authority_revision(&state)? != authorization
            || state.tokens.as_ref().is_some_and(|tokens| {
                !crate::auth::bearer::permits_personal_authority(tokens.access_token.expose())
            })
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        refresh::refresh_if_needed(self, &mut state)
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        // A refresh can lose its CAS to a different sign-in.
        if state.subject != *subject || &self.command_authority_revision(&state)? != authorization {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(state)
    }

    pub(super) async fn retained_resource_command(
        &self,
        access: &ResourceAccessService,
        envelope: &AgentCommandEnvelope,
    ) -> Result<CommandExecutionAuthorization, ResourceCommandAuthorizationError> {
        self.require_actor_owner_accounts(&envelope.actor, &envelope.owner)
            .await?;
        if envelope.authorization.authority_id.starts_with("binding:") {
            let context = self.retained_binding_context(envelope).await?;
            return Ok(access
                .authorize_command_execution(&context, "", envelope)
                .await?);
        }
        let state = self.retained_command_state(envelope).await?;
        let request = self.verify_command_custody(&state).await?;
        let mut context = self.resource_request_context(&request);
        // Authentication proved the actual subject. Preserve the admitted human
        // or API attribution without projecting the resource owner as a caller.
        context.actor = envelope.actor.clone();
        let credential = request
            .delegated_credential
            .as_deref()
            .ok_or(CommandAuthorizationError::Revoked)?;
        let proof = access
            .authorize_command_execution(&context, credential, envelope)
            .await?;
        // Provider membership and resource IO may overlap token replacement,
        // revocation or another sign-in, including one with the same token text.
        self.require_command_custody(&state).await?;
        proof.fence().require_current()?;
        Ok(proof)
    }
}
