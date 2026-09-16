//! The public setup port exposes safe status and owner inventory only.
use super::{
    custody::BridgeCustodyMetadata, managed_credentials::BridgeSecretRevocation,
    ManagedBridgeCredentials,
};
use crate::auth::persistence::SecretValue;
use layer2_core::bridge::{setup::*, BridgeError, BridgeId, BridgeService};
use layer2_core::resource_access::{CallerContext, ResourceOwner};

fn status(metadata: &BridgeCustodyMetadata) -> BridgeCredentialStatus {
    BridgeCredentialStatus {
        revision: metadata.revision,
        present: true,
        revoked: metadata.revoked_at.is_some(),
    }
}

#[async_trait::async_trait]
impl BridgeSetupCredentials for ManagedBridgeCredentials {
    async fn choices(
        &self,
        context: &CallerContext,
        credential: &str,
        owner: &ResourceOwner,
    ) -> Result<BridgeConnectorChoices, BridgeError> {
        // The core service authorizes this exact owner/admin before the port.
        if !context.validate_principal() {
            return Err(BridgeError::NotFound);
        }
        self.provider
            .choices(
                owner,
                &context.caller.subject,
                &SecretValue::new(credential.to_owned()),
            )
            .await
    }
    async fn status(
        &self,
        service: &BridgeService,
        context: &CallerContext,
        credential: Option<&str>,
        id: &BridgeId,
        expected_revision: u64,
    ) -> Result<BridgeCredentialStatus, BridgeError> {
        let lease = service
            .management_lease(context, credential, id, expected_revision)
            .await?;
        let selected = &lease.record().current()?.config.connector;
        let material = self.store.get(&selected.custody).await?;
        let result = if let Some(material) = material {
            material.metadata.validate()?;
            if material.metadata.bridge_id != *id
                || material.metadata.owner != lease.record().owner
                || material.metadata.connector != *selected
            {
                return Err(BridgeError::Corrupt);
            }
            status(&material.metadata)
        } else {
            BridgeCredentialStatus {
                revision: 0,
                present: false,
                revoked: false,
            }
        };
        service.validate_management(&lease).await?;
        Ok(result)
    }
    async fn import(
        &self,
        service: &BridgeService,
        context: &CallerContext,
        credential: Option<&str>,
        request: BridgeCredentialImport,
    ) -> Result<BridgeCredentialStatus, BridgeError> {
        let metadata =
            ManagedBridgeCredentials::import(self, service, context, credential, request).await?;
        Ok(status(&metadata))
    }
    async fn revoke(
        &self,
        service: &BridgeService,
        context: &CallerContext,
        credential: Option<&str>,
        request: BridgeCredentialRevocation,
    ) -> Result<BridgeCredentialStatus, BridgeError> {
        let lease = service
            .management_lease(context, credential, &request.id, request.expected_revision)
            .await?;
        let reference = lease.record().current()?.config.connector.custody.clone();
        let metadata = ManagedBridgeCredentials::revoke(
            self,
            service,
            context,
            credential,
            &BridgeSecretRevocation {
                bridge_id: request.id,
                expected_revision: request.expected_revision,
                reference,
                expected_custody_revision: request.expected_custody_revision,
                idempotency_key: request.idempotency_key,
            },
        )
        .await?;
        lease.require_current()?;
        Ok(status(&metadata))
    }
}

pub(crate) fn service(
    mode: &crate::auth::AuthMode,
    definitions: BridgeService,
    credentials: Option<std::sync::Arc<ManagedBridgeCredentials>>,
) -> Result<Option<std::sync::Arc<BridgeSetupService>>, BridgeError> {
    let crate::auth::AuthMode::Provider {
        base_url,
        public_url,
        ..
    } = mode
    else {
        return Ok(None);
    };
    let Some(credentials) = credentials else {
        return Ok(None);
    };
    if !reqwest::Url::parse(public_url).is_ok_and(|url| url.scheme() == "https") {
        return Ok(None);
    }
    let issuer = base_url.trim_end_matches('/').to_owned();
    let policy = BridgeSetupPolicy::new(
        issuer.clone(),
        issuer,
        format!(
            "{}/api/v1/bridge-callbacks",
            public_url.trim_end_matches('/')
        ),
    )?;
    Ok(Some(std::sync::Arc::new(BridgeSetupService::new(
        definitions,
        policy,
        credentials,
    ))))
}
