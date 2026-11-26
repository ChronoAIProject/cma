//! Execution of already admitted proxy commands preserves the recorded ceiling.
use super::super::persistence::SubjectAuthState;
use super::*;
use layer2_core::resource_access::{CallerContext, CredentialSource};
use layer2_core::sandbox::agent_io::{AgentAuthorizationRevision, CommandAuthorizationError};

pub(crate) struct ProxyCustody {
    pub grant: ProxyGrant,
    pub owner: SubjectAuthState,
}

fn unavailable(_: StoreError) -> CommandAuthorizationError {
    CommandAuthorizationError::Unavailable
}

fn authority(grant: &ProxyGrant) -> AgentAuthorizationRevision {
    AgentAuthorizationRevision {
        replay_principal: None,
        authority_id: format!("nyxid-proxy:{}", hex(&grant.id)),
        policy_revision: 1,
        grant_revision: None,
    }
}

/// New admissions scope replay by the stable principal when the gated writer
/// is enabled; execution still uses the exact grant in `authority_id`.
fn admission_authority(grant: &ProxyGrant, record_principal: bool) -> AgentAuthorizationRevision {
    AgentAuthorizationRevision {
        replay_principal: record_principal.then(|| replay_principal(grant)),
        ..authority(grant)
    }
}

fn hex(id: &[u8; 32]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

fn replay_principal(grant: &ProxyGrant) -> String {
    let bytes = serde_json::to_vec(&(
        "nyxid-proxy-replay-principal-v1",
        &grant.issuer,
        &grant.service_id,
        &grant.subject,
        &grant.owner_incarnation,
    ))
    .expect("typed replay principal is serializable");
    format!("nyxid-proxy-principal:{:x}", Sha256::digest(bytes))
}

fn parse_id(authority: &str) -> Result<[u8; 32], CommandAuthorizationError> {
    let text = authority
        .strip_prefix("nyxid-proxy:")
        .ok_or(CommandAuthorizationError::Revoked)?;
    if text.len() != 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(CommandAuthorizationError::Revoked);
    }
    let mut id = [0; 32];
    for (index, bytes) in text.as_bytes().chunks_exact(2).enumerate() {
        id[index] = u8::from_str_radix(
            std::str::from_utf8(bytes).map_err(|_| CommandAuthorizationError::Revoked)?,
            16,
        )
        .map_err(|_| CommandAuthorizationError::Revoked)?;
    }
    Ok(id)
}

impl AuthState {
    pub(in crate::auth) async fn proxy_command_authority(
        &self,
        context: &CallerContext,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        let CredentialSource::IdentityProxy { grant_digest } = context.credential_source else {
            return Err(CommandAuthorizationError::Revoked);
        };
        if !context.validate_principal()
            || !matches!(
                &context.actor,
                layer2_core::resource_access::ResourceActor::Api {
                    delegation_id: None,
                    ..
                }
            )
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        let custody = self
            .load_proxy_custody(&context.caller.subject, &grant_digest)
            .await?;
        let request = self.verify_proxy_custody(&custody).await?;
        if request.delegated_credential.as_deref() != Some(credential)
            || self
                .provider_credential_context(&request.subject, credential)
                .as_ref()
                != Some(&context.credential)
            || context
                .caller
                .authorization
                .iter()
                .any(|cap| !request.authorization.allows(cap))
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        self.require_proxy_custody(&custody).await?;
        Ok(admission_authority(
            &custody.grant,
            self.proxy_replay_principal_enabled,
        ))
    }

    /// Reader floor for a future gated writer that records the derived replay principal.
    pub(in crate::auth) async fn retained_proxy_custody(
        &self,
        subject: &SubjectId,
        revision: &AgentAuthorizationRevision,
    ) -> Result<ProxyCustody, CommandAuthorizationError> {
        let custody = self
            .load_proxy_custody(subject, &parse_id(&revision.authority_id)?)
            .await?;
        let expected = authority(&custody.grant);
        if revision.authority_id != expected.authority_id
            || revision.policy_revision != expected.policy_revision
            || revision.grant_revision != expected.grant_revision
            || revision
                .replay_principal
                .as_ref()
                .is_some_and(|principal| principal != &replay_principal(&custody.grant))
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(custody)
    }

    async fn load_proxy_custody(
        &self,
        subject: &SubjectId,
        id: &[u8; 32],
    ) -> Result<ProxyCustody, CommandAuthorizationError> {
        if self.identity_proxy.is_none() {
            return Err(CommandAuthorizationError::Revoked);
        }
        let grant = self
            .store
            .proxy_grant(id)
            .await
            .map_err(unavailable)?
            .ok_or(CommandAuthorizationError::Revoked)?;
        grant.validate().map_err(unavailable)?;
        if grant.subject != *subject
            || self.agent_key_service_id.as_deref() != Some(&grant.service_id)
            || !self
                .identity_proxy
                .as_ref()
                .is_some_and(|verifier| verifier.trusts(&grant.issuer, &grant.service_id))
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        let mut owner = self
            .store
            .subject(subject)
            .await
            .map_err(unavailable)?
            .ok_or(CommandAuthorizationError::Revoked)?;
        if owner.subject != *subject || owner.incarnation != grant.owner_incarnation {
            return Err(CommandAuthorizationError::Revoked);
        }
        refresh::refresh_if_needed(self, &mut owner)
            .await
            .map_err(unavailable)?;
        let custody = ProxyCustody { grant, owner };
        self.require_proxy_custody(&custody).await?;
        Ok(custody)
    }

    pub(in crate::auth) async fn verify_proxy_custody(
        &self,
        custody: &ProxyCustody,
    ) -> Result<RequestAuth, CommandAuthorizationError> {
        self.require_proxy_custody(custody).await?;
        let mut request = cookie_authorization::request_for_cookie(
            self,
            &custody.grant.subject,
            custody.owner.clone(),
        )
        .await
        .map_err(unavailable)?;
        request.authorization = intersect(&request.authorization, &custody.grant.authorization);
        request.credential_source = CredentialSource::IdentityProxy {
            grant_digest: custody.grant.id,
        };
        if request.delegated_credential.is_none() {
            return Err(CommandAuthorizationError::Revoked);
        }
        self.require_proxy_custody(custody).await?;
        Ok(request)
    }

    pub(in crate::auth) async fn require_proxy_custody(
        &self,
        custody: &ProxyCustody,
    ) -> Result<(), CommandAuthorizationError> {
        if !self.identity_proxy.as_ref().is_some_and(|verifier| {
            verifier.trusts(&custody.grant.issuer, &custody.grant.service_id)
        }) || custody.owner.subject != custody.grant.subject
            || custody.owner.incarnation != custody.grant.owner_incarnation
            || self
                .store
                .proxy_grant(&custody.grant.id)
                .await
                .map_err(unavailable)?
                .as_ref()
                != Some(&custody.grant)
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        cookie_authorization::require_current_custody(self, &custody.owner)
            .await
            .map_err(unavailable)
    }
}
