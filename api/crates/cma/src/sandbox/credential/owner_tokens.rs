//! Renewal and revocation use the exact owner consent for this scope revision.
use super::*;
use crate::{auth::AuthState, store::MongoSandboxStore};

pub(crate) struct ServiceAccessOwnerTokens {
    auth: Arc<AuthState>,
    store: Arc<MongoSandboxStore>,
    capture_retirement_authority: bool,
}

impl ServiceAccessOwnerTokens {
    pub(crate) fn new(auth: Arc<AuthState>, store: Arc<MongoSandboxStore>) -> Arc<Self> {
        Arc::new(Self {
            auth,
            store,
            capture_retirement_authority: false,
        })
    }

    pub(crate) fn with_retirement_authority_capture(
        mut self: Arc<Self>,
        enabled: bool,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("owner tokens configured before use")
            .capture_retirement_authority = enabled;
        self
    }
}

#[async_trait::async_trait]
impl OwnerTokenSource for ServiceAccessOwnerTokens {
    async fn token_for(&self, owner: &SubjectId) -> Result<Option<SecretValue>, String> {
        self.auth.token_for(owner).await
    }

    async fn token_for_sandbox(&self, sandbox: &Sandbox) -> Result<Option<SecretValue>, String> {
        let authority = self
            .store
            .credential_scope_authorization(sandbox)
            .await
            .map_err(|_| "service scope consent could not be read".to_owned())?;
        if let Some(authority) =
            authority.filter(|value| value.authority_id.starts_with("owner-api:"))
        {
            return self.auth.retained_owner_service_authority(&sandbox.owner, &authority).await
                .map(|(_, token)| Some(token))
                .map_err(|_| "owner_service_authority_required: renew consent with a current owner access token".into());
        }
        self.token_for(&sandbox.owner).await
    }

    async fn token_for_issuance(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<(SecretValue, Option<RetirementOwnerAuthority>)>, String> {
        if !self.capture_retirement_authority {
            return Ok(self
                .token_for_sandbox(sandbox)
                .await?
                .map(|token| (token, None)));
        }
        let consent = self
            .store
            .credential_scope_authorization(sandbox)
            .await
            .map_err(|_| "service scope consent could not be read".to_owned())?;
        if let Some(authorization) = consent
            .as_ref()
            .filter(|value| value.authority_id.starts_with("owner-api:"))
        {
            let (_, token) = self.auth.retained_owner_service_authority(&sandbox.owner, authorization).await
                .map_err(|_| "owner_service_authority_required: renew consent with a current owner access token".to_owned())?;
            return Ok(Some((
                token,
                Some(RetirementOwnerAuthority::OwnerApi {
                    owner: sandbox.owner.clone(),
                    authorization: authorization.clone(),
                }),
            )));
        }
        let delegated = self
            .auth
            .delegated_issuance_credential(&sandbox.owner)
            .await
            .map_err(|error| error.to_string())?;
        Ok(delegated.map(|(token, revision)| {
            let authority = revision
                .filter(|revision| consent.as_ref().is_none_or(|consent| consent == revision))
                .map(|authorization| RetirementOwnerAuthority::Personal {
                    owner: sandbox.owner.clone(),
                    authorization,
                });
            (token, authority)
        }))
    }
}
