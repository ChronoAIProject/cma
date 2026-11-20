//! Human deferred authority is bound to the sign-in incarnation that admitted
//! it. Refresh may rotate tokens within that incarnation; another sign-in may
//! not inherit old commands. No raw credential enters the command ledger.

use std::time::{Duration, SystemTime};

use layer2_core::sandbox::agent_io::{
    AgentActor, AgentAuthorizationRevision, AgentCommandAdmissionAuthority, AgentCommandAuthorizer,
    AgentCommandEnvelope, AgentOwner, CommandAuthorizationError, CommandExecutionAuthorization,
    PersonalCommandAdmissionAuthority, ResourceCommandAuthorizationError,
};
use layer2_core::{Caller, Capability};
use sha2::{Digest as _, Sha256};

use super::{
    cookie_authorization, persistence::SubjectAuthState, AuthMode, AuthState, RequestAuth,
};

mod api_bearer;
mod binding;
mod custody;
mod delegated;
mod owner_service_access;
mod personal_owner;
mod resource;
mod sandbox_controls;

const POLICY_REVISION: u64 = 1;
const VERIFICATION_TIMEOUT: Duration = Duration::from_secs(15);

#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for AuthState {
    async fn command_admission_authority(
        &self,
        context: &layer2_core::resource_access::CallerContext,
        credential: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        self.require_actor_owner_accounts(&context.actor, owner)
            .await?;
        if matches!(
            context.credential_source,
            layer2_core::resource_access::CredentialSource::IdentityProxy { .. }
        ) {
            return tokio::time::timeout(
                VERIFICATION_TIMEOUT,
                self.proxy_command_authority(context, credential),
            )
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        }
        if matches!(
            context.credential_source,
            layer2_core::resource_access::CredentialSource::RequestBoundRead { .. }
        ) {
            return Err(CommandAuthorizationError::Revoked);
        }
        if context.credential_source == layer2_core::resource_access::CredentialSource::Binding {
            return tokio::time::timeout(
                VERIFICATION_TIMEOUT,
                self.binding_admission_authority(context, credential, owner),
            )
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        }
        if !context.validate_principal()
            || self
                .provider_credential_context(&context.caller.subject, credential)
                .as_ref()
                != Some(&context.credential)
            || !matches!(
                &context.actor,
                AgentActor::Human { .. } | AgentActor::Api { .. }
            )
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        if matches!(
            &context.actor,
            AgentActor::Api {
                delegation_id: Some(_),
                ..
            }
        ) {
            return tokio::time::timeout(
                VERIFICATION_TIMEOUT,
                self.delegated_command_authority(context, credential),
            )
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        }
        if matches!(
            context.credential_source,
            layer2_core::resource_access::CredentialSource::Independent { .. }
        ) {
            return tokio::time::timeout(
                VERIFICATION_TIMEOUT,
                self.independent_command_authority(context, credential),
            )
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        }
        // Resource policy was independently checked by AgentCommandService.
        // Capture only this actual human's custody; the owner supplies no login.
        self.personal_command_authority(&context.caller, credential)
            .await
    }
}

#[async_trait::async_trait]
impl PersonalCommandAdmissionAuthority for AuthState {
    async fn owner_service_access_authority(
        &self,
        context: &layer2_core::resource_access::CallerContext,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        self.admit_owner_service_access(context, credential).await
    }

    async fn confirm_owner_service_scope(
        &self,
        context: &layer2_core::resource_access::CallerContext,
        credential: &str,
        services: &[String],
    ) -> Result<(), layer2_core::sandbox::agent_access::AgentAccessError> {
        self.verify_owner_service_scope(context, credential, services)
            .await
    }

    /// Bind admission to explicit credential custody. A bearer-only caller with
    /// no matching durable token cannot silently borrow another login's grant.
    async fn personal_command_authority(
        &self,
        caller: &Caller,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        self.require_command_account(&caller.subject).await?;
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
            self.verify_human_state(state).await?;
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
            replay_principal: None,
            authority_id: format!("personal:{:x}", Sha256::digest(bytes)),
            policy_revision: POLICY_REVISION,
            grant_revision: None,
        })
    }

    async fn verify_human_state(
        &self,
        state: SubjectAuthState,
    ) -> Result<RequestAuth, CommandAuthorizationError> {
        if state.tokens.as_ref().is_some_and(|tokens| {
            !super::bearer::permits_personal_authority(tokens.access_token.expose())
        }) {
            return Err(CommandAuthorizationError::Revoked);
        }
        let subject = state.subject.clone();
        let request = cookie_authorization::request_for_browser_cookie(self, &subject, state)
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?
            .ok_or(CommandAuthorizationError::Revoked)?;
        if request.delegated_credential.is_none()
            || !request.authorization.allows(Capability::UseOwnSandboxes)
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(request)
    }
}

#[async_trait::async_trait]
impl AgentCommandAuthorizer for AuthState {
    async fn authorize_command(
        &self,
        envelope: &AgentCommandEnvelope,
    ) -> Result<Caller, CommandAuthorizationError> {
        tokio::time::timeout(VERIFICATION_TIMEOUT, async {
            self.require_actor_owner_accounts(&envelope.actor, &envelope.owner)
                .await?;
            if envelope.authorization.authority_id.starts_with("binding:") {
                return Ok(self.retained_binding_context(envelope).await?.caller);
            }
            envelope
                .validate_integrity()
                .map_err(|_| CommandAuthorizationError::Revoked)?;
            let (
                AgentOwner::Person { subject: owner },
                AgentActor::Human { subject } | AgentActor::Api { subject, .. },
            ) = (&envelope.owner, &envelope.actor)
            else {
                return Err(CommandAuthorizationError::Revoked);
            };
            if owner != subject {
                return Err(CommandAuthorizationError::Revoked);
            }
            let state = self.retained_command_state(envelope).await?;
            let request = self.verify_command_custody(&state).await?;
            if !request.authorization.allows(Capability::UseOwnSandboxes) {
                return Err(CommandAuthorizationError::Revoked);
            }
            self.require_command_custody(&state).await?;
            Ok(Caller::new(request.subject, request.authorization))
        })
        .await
        .map_err(|_| CommandAuthorizationError::Unavailable)?
    }

    async fn authorize_resource_command(
        &self,
        access: &layer2_core::resource_access::ResourceAccessService,
        envelope: &AgentCommandEnvelope,
    ) -> Result<CommandExecutionAuthorization, ResourceCommandAuthorizationError> {
        tokio::time::timeout(
            VERIFICATION_TIMEOUT,
            self.retained_resource_command(access, envelope),
        )
        .await
        .map_err(|_| CommandAuthorizationError::Unavailable)?
    }

    async fn authorize_command_revival(
        &self,
        access: &layer2_core::resource_access::ResourceAccessService,
        command: &layer2_core::sandbox::agent_io::AdmittedAgentCommand,
    ) -> Result<
        std::sync::Arc<layer2_core::sandbox::agent_io::CommandRevivalAuthorization>,
        ResourceCommandAuthorizationError,
    > {
        tokio::time::timeout(
            VERIFICATION_TIMEOUT,
            self.retained_command_revival(access, command),
        )
        .await
        .map_err(|_| CommandAuthorizationError::Unavailable)?
    }
}
