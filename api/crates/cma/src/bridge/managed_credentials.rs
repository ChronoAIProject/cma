use std::sync::Arc;

use layer2_core::bridge::{
    BridgeConfig, BridgeError, BridgeId, BridgeSecretRef, BridgeService, BridgeSetupBlock,
    BridgeState, BridgeStore,
};
use layer2_core::resource_access::{CallerContext, ResourceOwner};

use super::{custody::*, provider::NyxBridgeProvider};
use crate::auth::persistence::SecretValue;

pub(crate) struct ManagedBridgeCredentials {
    pub store: Arc<dyn BridgeCredentialStore>,
    pub provider: Arc<NyxBridgeProvider>,
    pub bridges: Arc<dyn BridgeStore>,
    pub auth: Arc<crate::auth::AuthState>,
    pub provisioning: Option<Arc<super::provisioning::BridgeProvisioningDependencies>>,
}

pub(crate) struct BridgeSecretRevocation {
    pub bridge_id: BridgeId,
    pub expected_revision: u64,
    pub reference: BridgeSecretRef,
    pub expected_custody_revision: u64,
    pub idempotency_key: String,
}

impl ManagedBridgeCredentials {
    /// Only an owner/admin management lease admits secret material. Import is
    /// local to CMA and performs only read-only provider verification.
    #[allow(dead_code)] // The public bridge setup contract composes this next.
    pub async fn import(
        &self,
        service: &BridgeService,
        context: &CallerContext,
        credential: Option<&str>,
        request: layer2_core::bridge::setup::BridgeCredentialImport,
    ) -> Result<BridgeCustodyMetadata, BridgeError> {
        let id = &request.id;
        let expected_revision = request.expected_revision;
        let value = SecretValue::new(request.material.expose().to_owned());
        let mut management = service
            .management_lease(context, credential, id, expected_revision)
            .await?;
        if !matches!(management.record().state, BridgeState::Disabled { .. }) {
            return Err(BridgeError::InvalidConfiguration("bridge_must_be_disabled"));
        }
        if value.expose().is_empty() || value.expose().len() > 4096 {
            return Err(unavailable());
        }
        let record = management.record();
        let config = &record.current()?.config;
        let credential = credential.ok_or_else(super::control_plane::reconnect)?;
        let human = self
            .auth
            .capture_bridge_human(context, credential, &record.owner)
            .await?;
        let verified = self
            .provider
            .verify_secret(
                &record.owner,
                &value,
                &config.connector,
                human.subject(),
                human.credential(),
            )
            .await?;
        let now = std::time::SystemTime::now();
        let control_plane = request.retain_control_plane_authority.then(|| {
            super::control_plane::BridgeControlPlaneGrant {
                bridge_id: id.clone(),
                owner: record.owner.clone(),
                connector: config.connector.clone(),
                subject: human.subject().clone(),
                incarnation: human.incarnation().to_owned(),
                issued_at: layer2_core::rfc3339_at(now),
                expires_at: layer2_core::rfc3339_at(now + super::control_plane::GRANT_LIFETIME),
            }
        });
        let metadata = BridgeCustodyMetadata {
            bridge_id: id.clone(),
            owner: record.owner.clone(),
            connector: config.connector.clone(),
            revision: 1,
            key_state_version: verified.key_state_version,
            scopes: verified.scopes,
            allowed_service_ids: verified.allowed_service_ids,
            allow_all_services: verified.allow_all_services,
            allowed_node_ids: verified.allowed_node_ids,
            allow_all_nodes: verified.allow_all_nodes,
            created_at: layer2_core::now_rfc3339(),
            created_by: management.actor().clone(),
            revoked_at: None,
            revoked_by: None,
            revocation_key_digest: None,
        };
        metadata.validate()?;
        management.bound_deadline(verified.deadline);
        management.bound_deadline(human.deadline());
        service.validate_management(&management).await?;
        self.auth.revalidate_bridge_human(&human).await?;
        if tokio::time::Instant::now() >= verified.deadline {
            return Err(BridgeError::AuthorityUnavailable);
        }
        self.store
            .admit(
                &management,
                StoredBridgeCredential {
                    metadata,
                    value,
                    control_plane,
                },
            )
            .await
    }

    #[allow(dead_code)] // The public bridge setup contract composes this next.
    pub async fn revoke(
        &self,
        service: &BridgeService,
        context: &CallerContext,
        credential: Option<&str>,
        request: &BridgeSecretRevocation,
    ) -> Result<BridgeCustodyMetadata, BridgeError> {
        let id = &request.bridge_id;
        let expected_revision = request.expected_revision;
        let reference = &request.reference;
        let expected_custody_revision = request.expected_custody_revision;
        let idempotency_key = &request.idempotency_key;
        if idempotency_key.is_empty()
            || idempotency_key.len() > 200
            || !idempotency_key.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(BridgeError::InvalidConfiguration("idempotency_key"));
        }
        let management = service
            .management_lease(context, credential, id, expected_revision)
            .await?;
        let key_digest = layer2_core::profiles::content_digest(&(
            id,
            management.actor(),
            reference,
            expected_custody_revision,
            idempotency_key,
        ))
        .map_err(|_| BridgeError::Corrupt)?;
        self.store
            .revoke(
                &management,
                reference,
                expected_custody_revision,
                &key_digest,
            )
            .await
    }
}

#[async_trait::async_trait]
impl BridgeCredentialResolver for ManagedBridgeCredentials {
    async fn resolve(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeCredentialLease, BridgeError> {
        let material = self
            .store
            .get(&config.connector.custody)
            .await?
            .ok_or_else(unavailable)?;
        let metadata = &material.metadata;
        metadata.validate()?;
        if metadata.owner != *owner
            || metadata.connector != config.connector
            || metadata.revoked_at.is_some()
        {
            return Err(unavailable());
        }
        let bridge = self
            .bridges
            .bridge(&metadata.bridge_id)
            .await?
            .ok_or_else(unavailable)?;
        if bridge.owner != *owner
            || !bridge
                .generations
                .iter()
                .any(|generation| generation.config.connector == config.connector)
        {
            return Err(unavailable());
        }
        let control_plane = self.resolve_control_plane(owner, config).await?;
        let verified = self
            .provider
            .verify_secret(
                owner,
                &material.value,
                &config.connector,
                control_plane.actor_subject(),
                control_plane.credential(),
            )
            .await
            .inspect_err(|_| {
                super::operations::health::authority_failure(
                    super::operations::health::AuthorityFailure::CredentialVerificationFailed,
                );
            })?;
        if verified.key_state_version != metadata.key_state_version
            || verified.scopes != metadata.scopes
            || verified.allowed_service_ids != metadata.allowed_service_ids
            || verified.allow_all_services != metadata.allow_all_services
            || verified.allowed_node_ids != metadata.allowed_node_ids
            || verified.allow_all_nodes != metadata.allow_all_nodes
        {
            return Err(unavailable());
        }
        let lease = BridgeCredentialLease {
            owner: owner.clone(),
            credential_subject: super::types::owner_subject(owner).clone(),
            issuer: metadata.connector.issuer.clone(),
            key_id: metadata.connector.key_id.clone(),
            custody: metadata.connector.custody.clone(),
            custody_revision: metadata.revision,
            key_state_version: metadata.key_state_version,
            value: material.value,
            deadline: verified.deadline.min(control_plane.deadline()),
            routes: verified.routes,
            control_plane,
        };
        self.revalidate(&lease).await?;
        Ok(lease)
    }

    async fn revalidate(&self, lease: &BridgeCredentialLease) -> Result<(), BridgeError> {
        let current = self
            .store
            .get(&lease.custody)
            .await?
            .ok_or_else(unavailable)?;
        current.metadata.validate()?;
        if tokio::time::Instant::now() >= lease.deadline {
            super::operations::health::authority_failure(
                super::operations::health::AuthorityFailure::CredentialExpired,
            );
        }
        if current.metadata.owner != lease.owner
            || current.metadata.connector.key_id != lease.key_id
            || current.metadata.connector.issuer != lease.issuer
            || current.metadata.revision != lease.custody_revision
            || current.metadata.key_state_version != lease.key_state_version
            || current.metadata.revoked_at.is_some()
            || current.value != lease.value
            || tokio::time::Instant::now() >= lease.deadline
        {
            return Err(unavailable());
        }
        self.revalidate_control_plane(&lease.control_plane).await
    }
}

fn unavailable() -> BridgeError {
    BridgeError::ConnectionRequired(BridgeSetupBlock::CustodyUnavailable)
}
