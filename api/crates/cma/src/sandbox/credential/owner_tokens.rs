//! Renewal and revocation use the exact owner consent for this scope revision.
use super::*;
use crate::{auth::AuthState, store::MongoSandboxStore};
use layer2_core::{sandbox::RetirementAuthorityVersion, ServiceId};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

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
    async fn retirement_baseline(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<RetirementAuthorityVersion>, CredentialError> {
        if !self.auth.service_authorization_recovery_available() {
            return Ok(None);
        }
        let generation = self
            .auth
            .retirement_token_generation(&sandbox.owner)
            .await
            .map_err(|_| evidence_unavailable())?;
        Ok(Some(RetirementAuthorityVersion {
            grant: [0; 16],
            scope: retirement_scope(&retirement_services(sandbox)),
            granted_at_ms: 0,
            token_generation: Some(generation),
        }))
    }

    async fn retirement_token(&self, sandbox: &Sandbox) -> Result<Option<SecretValue>, String> {
        self.token_for(&sandbox.owner).await
    }

    async fn retirement_authority_version(
        &self,
        sandbox: &Sandbox,
        token: &SecretValue,
    ) -> Result<Option<RetirementAuthorityVersion>, CredentialError> {
        let required = retirement_services(sandbox);
        let observed = self
            .auth
            .retirement_authority_version(&sandbox.owner, token, &required)
            .await
            .map_err(|_| evidence_unavailable())?;
        Ok(observed.map(|version| RetirementAuthorityVersion {
            scope: retirement_scope(&required),
            ..version
        }))
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

fn retirement_services(sandbox: &Sandbox) -> BTreeSet<ServiceId> {
    let selected = sandbox.credential.selected_connection();
    sandbox
        .credential
        .effective_reaches()
        .into_iter()
        .map(|service| {
            selected
                .filter(|connection| connection.service == service)
                .map_or(service, |connection| {
                    ServiceId::new(connection.connection_id.clone())
                })
        })
        .collect()
}

fn retirement_scope(required: &BTreeSet<ServiceId>) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"cma-retirement-service-scope-v1");
    for service in required {
        let value = service.as_str().as_bytes();
        digest.update((value.len() as u64).to_be_bytes());
        digest.update(value);
    }
    digest.finalize().into()
}

fn evidence_unavailable() -> CredentialError {
    CredentialError::Unavailable("owner reauthorization evidence is unavailable".into())
}
