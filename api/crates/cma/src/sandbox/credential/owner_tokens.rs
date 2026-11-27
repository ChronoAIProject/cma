//! Renewal and revocation use the exact owner consent for this scope revision.
use super::*;
use crate::{auth::AuthState, store::MongoSandboxStore};

pub(crate) struct ServiceAccessOwnerTokens {
    auth: Arc<AuthState>,
    store: Arc<MongoSandboxStore>,
}

impl ServiceAccessOwnerTokens {
    pub(crate) fn new(auth: Arc<AuthState>, store: Arc<MongoSandboxStore>) -> Arc<Self> {
        Arc::new(Self { auth, store })
    }
}

#[async_trait::async_trait]
impl OwnerTokenSource for ServiceAccessOwnerTokens {
    async fn retirement_authority_version(
        &self,
        sandbox: &Sandbox,
        token: &SecretValue,
    ) -> Result<Option<layer2_core::sandbox::RetirementAuthorityVersion>, CredentialError> {
        self.auth
            .retirement_authority_version(&sandbox.owner, token)
            .await
            .map_err(|_| {
                CredentialError::Unavailable("owner reauthorization evidence is unavailable".into())
            })
    }

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
}
