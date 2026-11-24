//! Direct owner API custody for service grants; runtime keys cannot self-escalate.
use super::*;
use crate::auth::{
    keys::{KeyIssuer, KeyIssuerError},
    persistence::SecretValue,
};
use layer2_core::resource_access::{CallerContext, CredentialSource};
use layer2_core::sandbox::agent_access::{validate_access_services, AgentAccessError};
use layer2_core::{ServiceId, SubjectId};

impl AuthState {
    pub(super) async fn admit_owner_service_access(
        &self,
        context: &CallerContext,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if !context.validate_principal()
            || !context
                .caller
                .authorization
                .allows(Capability::UseOwnSandboxes)
            || self
                .provider_credential_context(&context.caller.subject, credential)
                .as_ref()
                != Some(&context.credential)
            || !matches!(&context.actor, AgentActor::Human { subject } | AgentActor::Api { subject, delegation_id: None } if subject == &context.caller.subject)
            || !super::super::bearer::permits_personal_authority(credential)
            || super::super::agent_keys::is_agent_key(credential)
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        if context.credential_source == CredentialSource::Retained {
            return self
                .personal_command_authority(&context.caller, credential)
                .await;
        }
        if !matches!(
            context.credential_source,
            CredentialSource::Independent {
                expires_at: Some(_)
            }
        ) {
            return Err(CommandAuthorizationError::Revoked);
        }
        let mut authority = tokio::time::timeout(
            VERIFICATION_TIMEOUT,
            self.independent_command_authority(context, credential),
        )
        .await
        .map_err(|_| CommandAuthorizationError::Unavailable)??;
        // A separate namespace prevents ordinary Agent-key command custody
        // from being mistaken for owner service-management consent.
        authority.authority_id = format!("owner-{}", authority.authority_id);
        Ok(authority)
    }

    pub async fn retained_owner_service_authority(
        &self,
        owner: &SubjectId,
        authorization: &AgentAuthorizationRevision,
    ) -> Result<(CallerContext, SecretValue), CommandAuthorizationError> {
        if authorization.authority_id.starts_with("personal:") {
            return self
                .retained_personal_owner_authority(owner, authorization)
                .await;
        }
        tokio::time::timeout(VERIFICATION_TIMEOUT, async {
            let mut api = authorization.clone();
            api.authority_id = api.authority_id.strip_prefix("owner-")
                .filter(|value| value.starts_with("api:"))
                .ok_or(CommandAuthorizationError::Revoked)?.into();
            let custody = self.retained_api_custody(owner, &api).await?;
            if super::super::agent_keys::is_agent_key(custody.access_token.expose())
                || !super::super::bearer::permits_personal_authority(custody.access_token.expose())
                || custody.expires_at.is_none()
            { return Err(CommandAuthorizationError::Revoked); }
            let request = self.verify_api_custody(&custody).await?;
            let context = self.resource_request_context(&request);
            if !context.validate_principal()
                || context.caller.subject != *owner
                || context.credential.credential_id != custody.credential_id
                || !matches!(&context.actor, AgentActor::Api { subject, delegation_id: None } if subject == owner)
            { return Err(CommandAuthorizationError::Revoked); }
            self.require_api_custody(&custody).await?;
            Ok((context, custody.access_token.clone()))
        }).await.map_err(|_| CommandAuthorizationError::Unavailable)?
    }

    pub(super) async fn verify_owner_service_scope(
        &self,
        context: &CallerContext,
        credential: &str,
        services: &[String],
    ) -> Result<(), AgentAccessError> {
        validate_access_services(services)?;
        if context.credential_source == CredentialSource::Retained {
            return Ok(());
        }
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return Err(AgentAccessError::OwnerAuthorityRequired);
        };
        self.admit_owner_service_access(context, credential)
            .await
            .map_err(|error| match error {
                CommandAuthorizationError::Revoked => AgentAccessError::OwnerAuthorityRequired,
                CommandAuthorizationError::Unavailable => AgentAccessError::Unavailable,
            })?;
        let issuer = KeyIssuer::new(self.http.clone(), base_url, "scope-verification");
        tokio::time::timeout(
            VERIFICATION_TIMEOUT,
            issuer.confirm_service_scope(
                &context.caller.subject,
                credential,
                &services
                    .iter()
                    .cloned()
                    .map(ServiceId::new)
                    .collect::<Vec<_>>(),
            ),
        )
        .await
        .map_err(|_| AgentAccessError::Unavailable)?
        .map_err(|error| match error {
            KeyIssuerError::ServiceNotGranted(_) | KeyIssuerError::Refused(400 | 403 | 409) => {
                AgentAccessError::ServiceConnectionRequired
            }
            KeyIssuerError::Refused(401) => AgentAccessError::OwnerAuthorityRequired,
            _ => AgentAccessError::Unavailable,
        })
    }
}
