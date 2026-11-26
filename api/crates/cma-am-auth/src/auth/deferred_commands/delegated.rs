//! Retained commands preserve the exact explicitly approved actor delegation.
use super::*;
use crate::auth::actor_links::{ActorGrant, ActorGrantScope, EventActor};
use layer2_core::resource_access::{CallerContext, CredentialSource};

pub(super) struct DelegatedCommandCustody {
    state: SubjectAuthState,
    grant: ActorGrant,
    actor: EventActor,
}

impl AuthState {
    pub(super) async fn delegated_command_authority(
        &self,
        context: &CallerContext,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        let AgentActor::Api {
            subject,
            delegation_id: Some(grant_id),
        } = &context.actor
        else {
            return Err(CommandAuthorizationError::Revoked);
        };
        if context.credential_source != CredentialSource::delegated(grant_id) {
            return Err(CommandAuthorizationError::Revoked);
        }
        let custody = self.delegated_custody(subject, grant_id).await?;
        if !custody.state.tokens.as_ref().is_some_and(|tokens| {
            tokens.access_token.expose() == credential && tokens.expires_at > SystemTime::now()
        }) {
            return Err(CommandAuthorizationError::Revoked);
        }
        let authority = self.delegated_authority_revision(&custody)?;
        self.verify_delegated_custody(&custody).await?;
        Ok(authority)
    }

    pub(super) async fn retained_delegated_custody(
        &self,
        subject: &layer2_core::SubjectId,
        grant_id: &str,
        authority: &AgentAuthorizationRevision,
    ) -> Result<DelegatedCommandCustody, CommandAuthorizationError> {
        let custody = self.delegated_custody(subject, grant_id).await?;
        if &self.delegated_authority_revision(&custody)? != authority {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(custody)
    }

    async fn delegated_custody(
        &self,
        subject: &layer2_core::SubjectId,
        grant_id: &str,
    ) -> Result<DelegatedCommandCustody, CommandAuthorizationError> {
        let row = self.link_record(grant_id).await.map_err(command_error)?;
        let grant = row.grant().map_err(command_error)?;
        if grant.subject != *subject || grant.scope != ActorGrantScope::OrdinaryCma {
            return Err(CommandAuthorizationError::Revoked);
        }
        let mut state = self
            .store
            .subject(subject)
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?
            .ok_or(CommandAuthorizationError::Revoked)?;
        state
            .validate()
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        if state.subject != *subject || state.incarnation != grant.credential_incarnation {
            return Err(CommandAuthorizationError::Revoked);
        }
        crate::auth::refresh::refresh_if_needed(self, &mut state)
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        let custody = DelegatedCommandCustody {
            state,
            grant,
            actor: row.actor,
        };
        self.require_delegated_custody(&custody).await?;
        Ok(custody)
    }

    fn delegated_authority_revision(
        &self,
        custody: &DelegatedCommandCustody,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return Err(CommandAuthorizationError::Revoked);
        };
        let bytes = serde_json::to_vec(&(
            "delegated-command-authority-v1",
            base_url.trim_end_matches('/'),
            &custody.grant.subject,
            &custody.grant.credential_incarnation,
            &custody.grant.grant_id,
            &custody.actor,
        ))
        .map_err(|_| CommandAuthorizationError::Unavailable)?;
        Ok(AgentAuthorizationRevision {
            replay_principal: None,
            authority_id: format!("delegated:{:x}", Sha256::digest(bytes)),
            policy_revision: POLICY_REVISION,
            grant_revision: None,
        })
    }

    pub(super) async fn verify_delegated_custody(
        &self,
        custody: &DelegatedCommandCustody,
    ) -> Result<RequestAuth, CommandAuthorizationError> {
        self.require_delegated_custody(custody).await?;
        let mut request = self.verify_human_state(custody.state.clone()).await?;
        request.credential_source = CredentialSource::delegated(&custody.grant.grant_id);
        self.require_delegated_custody(custody).await?;
        Ok(request)
    }

    pub(super) async fn require_delegated_custody(
        &self,
        custody: &DelegatedCommandCustody,
    ) -> Result<(), CommandAuthorizationError> {
        let current = self
            .link_record(&custody.grant.grant_id)
            .await
            .map_err(command_error)?;
        let grant = current.grant().map_err(command_error)?;
        if current.actor != custody.actor
            || grant.scope != ActorGrantScope::OrdinaryCma
            || grant.subject != custody.grant.subject
            || grant.credential_incarnation != custody.grant.credential_incarnation
            || custody.state.subject != grant.subject
            || custody.state.incarnation != grant.credential_incarnation
            || custody.state.tokens.as_ref().is_none_or(|tokens| {
                tokens
                    .refresh_token
                    .as_ref()
                    .is_none_or(|token| token.expose().is_empty())
                    || !crate::auth::bearer::permits_personal_authority(
                        tokens.access_token.expose(),
                    )
            })
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        cookie_authorization::require_current_custody(self, &custody.state)
            .await
            // A concurrent token refresh can change custody without revoking
            // this grant. Retry with the current generation; the next read
            // independently rejects a replaced incarnation or revoked grant.
            .map_err(|_| CommandAuthorizationError::Unavailable)
    }
}

fn command_error(error: layer2_core::bots::BotError) -> CommandAuthorizationError {
    match error {
        layer2_core::bots::BotError::Unavailable | layer2_core::bots::BotError::Conflict => {
            CommandAuthorizationError::Unavailable
        }
        _ => CommandAuthorizationError::Revoked,
    }
}
