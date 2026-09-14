use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::agent_io::AgentSourceKind;

use super::*;
use crate::auth::refresh;

impl AuthState {
    /// Refresh only the sign-in that admitted the command. The revision is
    /// credential custody, independent of resource ownership and sharing policy.
    pub(super) async fn retained_command_state(
        &self,
        envelope: &AgentCommandEnvelope,
    ) -> Result<SubjectAuthState, CommandAuthorizationError> {
        envelope
            .validate_integrity()
            .map_err(|_| CommandAuthorizationError::Revoked)?;
        let subject = match &envelope.actor {
            AgentActor::Human { subject }
            | AgentActor::Api {
                subject,
                delegation_id: None,
            } if matches!(
                envelope.source.kind,
                AgentSourceKind::Api | AgentSourceKind::UserInterface
            ) =>
            {
                subject
            }
            _ => return Err(CommandAuthorizationError::Revoked),
        };
        let mut state = self
            .store
            .subject(subject)
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?
            .ok_or(CommandAuthorizationError::Revoked)?;
        if state.subject != *subject
            || self.command_authority_revision(&state)? != envelope.authorization
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
        if state.subject != *subject
            || self.command_authority_revision(&state)? != envelope.authorization
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(state)
    }

    pub(super) async fn retained_resource_command(
        &self,
        access: &ResourceAccessService,
        envelope: &AgentCommandEnvelope,
    ) -> Result<CommandExecutionAuthorization, ResourceCommandAuthorizationError> {
        let state = self.retained_command_state(envelope).await?;
        let request = self.verify_human_state(state.clone()).await?;
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
        cookie_authorization::require_current_custody(self, &state)
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        proof.fence().require_current()?;
        Ok(proof)
    }
}
