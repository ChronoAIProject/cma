//! An API credential is held independently; never borrowed from a browser login.

use super::*;
use crate::auth::{
    bearer_authorization,
    permissions::PermissionEvidence,
    persistence::{ApiBearerCustody, SecretDigest, SecretValue},
    verification_error::ProviderVerificationError,
};
use layer2_core::resource_access::{CallerContext, CredentialSource};

impl AuthState {
    pub async fn independent_command_authority(
        &self,
        context: &CallerContext,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if !matches!(
            context.actor,
            AgentActor::Api {
                delegation_id: None,
                ..
            }
        ) {
            return Err(CommandAuthorizationError::Revoked);
        }
        let request = self
            .verify_independent_bearer(&context.caller.subject, credential)
            .await?;
        if request.credential_source != context.credential_source {
            return Err(CommandAuthorizationError::Revoked);
        }
        let CredentialSource::Independent { expires_at } = request.credential_source else {
            return Err(CommandAuthorizationError::Revoked);
        };
        let record = ApiBearerCustody::new(
            context.credential.issuer.clone(),
            context.caller.subject.clone(),
            context.credential.credential_id.clone(),
            SecretValue::new(credential.to_owned()),
            expires_at,
        )
        .map_err(|_| CommandAuthorizationError::Revoked)?;
        self.store
            .insert_api_bearer(record.clone())
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        self.require_api_custody(&record).await?;
        api_authority(&record)
    }

    pub async fn retained_api_custody(
        &self,
        subject: &layer2_core::SubjectId,
        authority: &AgentAuthorizationRevision,
    ) -> Result<ApiBearerCustody, CommandAuthorizationError> {
        let id = api_custody_id(&authority.authority_id)?;
        let record = self
            .store
            .api_bearer(&id)
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?
            .ok_or(CommandAuthorizationError::Revoked)?;
        record
            .validate()
            .map_err(|_| CommandAuthorizationError::Revoked)?;
        if record.subject != *subject
            || &api_authority(&record)? != authority
            || self
                .provider_credential_context(subject, record.access_token.expose())
                .is_none_or(|context| {
                    context.issuer != record.issuer || context.credential_id != record.credential_id
                })
            || record
                .expires_at
                .is_some_and(|expiry| expiry <= SystemTime::now())
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(record)
    }

    pub async fn verify_api_custody(
        &self,
        record: &ApiBearerCustody,
    ) -> Result<RequestAuth, CommandAuthorizationError> {
        self.require_api_custody(record).await?;
        let request = self
            .verify_independent_bearer(&record.subject, record.access_token.expose())
            .await?;
        if request.credential_source
            != (CredentialSource::Independent {
                expires_at: record.expires_at,
            })
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        self.require_api_custody(record).await?;
        Ok(request)
    }

    pub(in crate::auth) async fn require_api_custody(
        &self,
        record: &ApiBearerCustody,
    ) -> Result<(), CommandAuthorizationError> {
        if record
            .expires_at
            .is_some_and(|expiry| expiry <= SystemTime::now())
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        let current = self
            .store
            .api_bearer(&record.id)
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        if current.as_ref() != Some(record)
            || record
                .expires_at
                .is_some_and(|expiry| expiry <= SystemTime::now())
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(())
    }

    async fn verify_independent_bearer(
        &self,
        subject: &layer2_core::SubjectId,
        credential: &str,
    ) -> Result<RequestAuth, CommandAuthorizationError> {
        if !self.permits_oauth_or_key_commands(credential) {
            return Err(CommandAuthorizationError::Revoked);
        }
        self.require_ordinary_api_credential(credential).await?;
        let metadata = self
            .metadata()
            .await
            .map_err(|_| CommandAuthorizationError::Unavailable)?;
        let verified = bearer_authorization::verify_subject(self, &metadata, credential)
            .await
            .map_err(ProviderVerificationError::command_error)?;
        let PermissionEvidence::Active(permissions) = &verified.permissions else {
            return Err(CommandAuthorizationError::Revoked);
        };
        if !verified.independent_commands
            || verified
                .api_expires_at
                .is_some_and(|expiry| expiry <= SystemTime::now())
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        let expires_at = verified.api_expires_at;
        let authorization =
            crate::permissions::capabilities_from_permissions(permissions.iter().cloned());
        if verified.subject != *subject || !authorization.allows(Capability::UseOwnSandboxes) {
            return Err(CommandAuthorizationError::Revoked);
        }
        // Binding registration can race provider IO. A key that acquired a
        // restricted binding identity cannot retain earlier ambient authority.
        self.require_ordinary_api_credential(credential).await?;
        Ok(RequestAuth {
            binding: None,
            credential_source: CredentialSource::Independent { expires_at },
            subject: verified.subject.clone(),
            authorization,
            profile: verified.profile.clone(),
            delegated_credential: Some(credential.to_owned()),
        })
    }

    async fn require_ordinary_api_credential(
        &self,
        credential: &str,
    ) -> Result<(), CommandAuthorizationError> {
        if crate::auth::agent_keys::is_agent_key(credential)
            && self
                .is_known_binding_key(credential)
                .await
                .map_err(|_| CommandAuthorizationError::Unavailable)?
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(())
    }
}

fn api_authority(
    record: &ApiBearerCustody,
) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
    let expiry = record
        .expires_at
        .map(|expiry| {
            expiry
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|value| value.as_secs().to_string())
                .map_err(|_| CommandAuthorizationError::Revoked)
        })
        .transpose()?
        .unwrap_or_else(|| "none".into());
    let id: String = record
        .id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok(AgentAuthorizationRevision {
        replay_principal: None,
        authority_id: format!("api:{id}:{expiry}"),
        policy_revision: POLICY_REVISION,
        grant_revision: None,
    })
}

fn api_custody_id(authority: &str) -> Result<SecretDigest, CommandAuthorizationError> {
    let (hex, _) = authority
        .strip_prefix("api:")
        .and_then(|rest| rest.split_once(':'))
        .ok_or(CommandAuthorizationError::Revoked)?;
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(CommandAuthorizationError::Revoked);
    }
    let mut bytes = [0; 32];
    for (index, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let chunk = std::str::from_utf8(chunk).map_err(|_| CommandAuthorizationError::Revoked)?;
        bytes[index] =
            u8::from_str_radix(chunk, 16).map_err(|_| CommandAuthorizationError::Revoked)?;
    }
    Ok(SecretDigest::from_bytes(bytes))
}
