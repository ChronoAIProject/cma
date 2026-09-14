//! Personal deferred authority is bound to the sign-in incarnation that admitted
//! it. Refresh may rotate tokens within that incarnation; another sign-in may
//! not inherit old commands. No raw credential enters the command ledger.

use std::time::{Duration, SystemTime};

use layer2_core::sandbox::agent_io::{
    AgentActor, AgentAuthorizationRevision, AgentCommandAuthorizer, AgentCommandEnvelope,
    AgentOwner, CommandAuthorizationError, PersonalCommandAdmissionAuthority,
};
use layer2_core::{Caller, Capability};
use sha2::{Digest as _, Sha256};

use super::{cookie_authorization, persistence::SubjectAuthState, refresh, AuthMode, AuthState};

const POLICY_REVISION: u64 = 1;
const VERIFICATION_TIMEOUT: Duration = Duration::from_secs(15);

#[async_trait::async_trait]
impl PersonalCommandAdmissionAuthority for AuthState {
    /// Bind admission to explicit credential custody. A bearer-only caller with
    /// no matching durable token cannot silently borrow another login's grant.
    async fn personal_command_authority(
        &self,
        caller: &Caller,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        tokio::time::timeout(VERIFICATION_TIMEOUT, async {
            let state = self
                .store
                .subject(&caller.subject)
                .await
                .map_err(|_| CommandAuthorizationError::Unavailable)?
                .ok_or(CommandAuthorizationError::Revoked)?;
            if state.subject != caller.subject
                || !state.tokens.as_ref().is_some_and(|tokens| {
                    tokens.access_token.expose() == credential
                        && tokens.expires_at > SystemTime::now()
                })
            {
                return Err(CommandAuthorizationError::Revoked);
            }
            let authority = self.command_authority_revision(&state)?;
            self.verify_personal_state(state).await?;
            Ok(authority)
        })
        .await
        .map_err(|_| CommandAuthorizationError::Unavailable)?
    }
}

impl AuthState {
    fn command_authority_revision(
        &self,
        state: &SubjectAuthState,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        state
            .validate()
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return Err(CommandAuthorizationError::Revoked);
        };
        let bytes = serde_json::to_vec(&(
            "personal-command-authority-v1",
            base_url.trim_end_matches('/'),
            &state.subject,
            &state.incarnation,
        ))
        .map_err(|_| CommandAuthorizationError::Unavailable)?;
        Ok(AgentAuthorizationRevision {
            authority_id: format!("personal:{:x}", Sha256::digest(bytes)),
            policy_revision: POLICY_REVISION,
            grant_revision: None,
        })
    }

    async fn verify_personal_state(
        &self,
        state: SubjectAuthState,
    ) -> Result<Caller, CommandAuthorizationError> {
        if state.tokens.as_ref().is_some_and(|tokens| {
            !super::bearer::permits_personal_authority(tokens.access_token.expose())
        }) {
            return Err(CommandAuthorizationError::Revoked);
        }
        let subject = state.subject.clone();
        let request = cookie_authorization::request_for_cookie(self, &subject, state)
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        if request.delegated_credential.is_none()
            || !request.authorization.allows(Capability::UseOwnSandboxes)
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(Caller::new(request.subject, request.authorization))
    }
}

#[async_trait::async_trait]
impl AgentCommandAuthorizer for AuthState {
    async fn authorize_command(
        &self,
        envelope: &AgentCommandEnvelope,
    ) -> Result<Caller, CommandAuthorizationError> {
        tokio::time::timeout(VERIFICATION_TIMEOUT, async {
            envelope
                .validate_integrity()
                .map_err(|_| CommandAuthorizationError::Revoked)?;
            let (
                AgentOwner::Person { subject: owner },
                AgentActor::Human { subject }
                | AgentActor::Api {
                    subject,
                    delegation_id: None,
                },
            ) = (&envelope.owner, &envelope.actor)
            else {
                return Err(CommandAuthorizationError::Revoked);
            };
            if owner != subject {
                return Err(CommandAuthorizationError::Revoked);
            }
            let mut state = self
                .store
                .subject(subject)
                .await
                .map_err(|_| CommandAuthorizationError::Unavailable)?
                .ok_or(CommandAuthorizationError::Revoked)?;
            if state.subject != *subject
                || self.command_authority_revision(&state)? != envelope.authorization
            {
                return Err(CommandAuthorizationError::Revoked);
            }
            refresh::refresh_if_needed(self, &mut state)
                .await
                .map_err(|_| CommandAuthorizationError::Unavailable)?;
            // Refresh can lose a CAS to a new sign-in. Never adopt its authority.
            if state.subject != *subject
                || self.command_authority_revision(&state)? != envelope.authorization
            {
                return Err(CommandAuthorizationError::Revoked);
            }
            self.verify_personal_state(state).await
        })
        .await
        .map_err(|_| CommandAuthorizationError::Unavailable)?
    }
}
