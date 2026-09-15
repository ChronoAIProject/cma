//! Bridge custody deliberately does not reuse ambient human subject tokens.
//! Like Sandbox CredentialMaterial, leases contain only a selected secret in
//! redacted, zeroizing SecretValue and immutable authority identifiers. A real
//! resolver must read an encrypted, owner-bound record and verify its active
//! provider key before returning a lease; raw API-key IDs are not credentials.

use layer2_core::bridge::{BridgeConfig, BridgeError, BridgeSecretRef};
use layer2_core::resource_access::ResourceOwner;
use layer2_core::SubjectId;

use crate::auth::persistence::SecretValue;

#[derive(Debug)]
pub(crate) struct BridgeCredentialLease {
    pub owner: ResourceOwner,
    /// Verified subject of this exact retained credential. An organization
    /// lease must authenticate as that organization, never its configuring admin.
    pub credential_subject: SubjectId,
    pub issuer: String,
    pub key_id: String,
    pub custody: BridgeSecretRef,
    pub custody_revision: u64,
    pub key_state_version: i64,
    pub value: SecretValue,
    pub deadline: tokio::time::Instant,
}

#[async_trait::async_trait]
pub(crate) trait BridgeCredentialResolver: Send + Sync {
    async fn resolve(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeCredentialLease, BridgeError>;
    /// Recheck the same retained custody revision after provider/target IO.
    /// Implementations must not swap in an unrelated replacement key on retry.
    async fn revalidate(&self, lease: &BridgeCredentialLease) -> Result<(), BridgeError>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BridgeCustodyMetadata {
    pub bridge_id: layer2_core::bridge::BridgeId,
    pub owner: ResourceOwner,
    pub connector: layer2_core::bridge::BridgeConnector,
    pub revision: u64,
    pub key_state_version: i64,
    pub scopes: String,
    pub allowed_service_ids: Vec<String>,
    pub allow_all_services: bool,
    pub allowed_node_ids: Vec<String>,
    pub allow_all_nodes: bool,
    pub created_at: String,
    pub created_by: layer2_core::resource_access::ResourceActor,
    pub revoked_at: Option<String>,
    pub revoked_by: Option<layer2_core::resource_access::ResourceActor>,
    pub revocation_key_digest: Option<String>,
}

impl BridgeCustodyMetadata {
    pub fn validate(&self) -> Result<(), BridgeError> {
        let bad = || BridgeError::Corrupt;
        BridgeSecretRef::new(self.connector.custody.as_str().to_owned()).map_err(|_| bad())?;
        if !super::types::canonical_id(self.bridge_id.as_str())
            || !super::types::canonical_id(super::types::owner_subject(&self.owner).as_str())
            || !super::types::canonical_id(&self.connector.key_id)
            || !super::types::canonical_id(&self.connector.bot_id)
            || self.key_state_version <= 0
            || self.scopes.len() > 256
            || self.allowed_service_ids.len() > 1000
            || self.allowed_node_ids.len() > 1000
            || self
                .allowed_service_ids
                .iter()
                .chain(&self.allowed_node_ids)
                .any(|id| !super::types::canonical_id(id))
            || layer2_core::seconds_from_rfc3339(&self.created_at).is_none()
        {
            return Err(bad());
        }
        match (
            self.revision,
            &self.revoked_at,
            &self.revoked_by,
            &self.revocation_key_digest,
        ) {
            (1, None, None, None) => {}
            (2, Some(at), Some(_), Some(key))
                if layer2_core::seconds_from_rfc3339(at).is_some()
                    && key.len() == 64
                    && key.bytes().all(|b| b.is_ascii_hexdigit()) => {}
            _ => return Err(bad()),
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct StoredBridgeCredential {
    pub metadata: BridgeCustodyMetadata,
    pub value: SecretValue,
}

#[async_trait::async_trait]
pub(crate) trait BridgeCredentialStore: Send + Sync {
    async fn get(
        &self,
        reference: &BridgeSecretRef,
    ) -> Result<Option<StoredBridgeCredential>, BridgeError>;
    async fn admit(
        &self,
        management: &layer2_core::bridge::BridgeManagementLease,
        material: StoredBridgeCredential,
    ) -> Result<BridgeCustodyMetadata, BridgeError>;
    async fn revoke(
        &self,
        management: &layer2_core::bridge::BridgeManagementLease,
        reference: &BridgeSecretRef,
        expected_revision: u64,
        key_digest: &str,
    ) -> Result<BridgeCustodyMetadata, BridgeError>;
}
