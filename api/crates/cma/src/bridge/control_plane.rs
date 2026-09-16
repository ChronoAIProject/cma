//! A deliberate, bounded grant references existing encrypted OAuth custody.
//! It is stored separately from selected-key metadata so older custody remains
//! readable and fails closed until the manager explicitly reconnects it.
use std::time::{Duration, SystemTime};

use layer2_core::bridge::{BridgeConfig, BridgeConnector, BridgeError, BridgeId, BridgeSetupBlock};
use layer2_core::resource_access::ResourceOwner;
use layer2_core::SubjectId;

use super::ManagedBridgeCredentials;
use crate::auth::bridge_control_plane::BridgeHumanLease;

pub(crate) mod invoke;

pub(crate) const GRANT_LIFETIME: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BridgeControlPlaneGrant {
    pub bridge_id: BridgeId,
    pub owner: ResourceOwner,
    pub connector: BridgeConnector,
    pub subject: SubjectId,
    pub incarnation: String,
    pub issued_at: String,
    pub expires_at: String,
}

impl std::fmt::Debug for BridgeControlPlaneGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeControlPlaneGrant")
            .field("bridge_id", &self.bridge_id)
            .field("owner", &self.owner)
            .field("subject", &self.subject)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl BridgeControlPlaneGrant {
    pub(crate) fn validate(&self) -> Result<(), BridgeError> {
        let issued =
            layer2_core::seconds_from_rfc3339(&self.issued_at).ok_or(BridgeError::Corrupt)?;
        let expiry =
            layer2_core::seconds_from_rfc3339(&self.expires_at).ok_or(BridgeError::Corrupt)?;
        if !super::types::canonical_id(self.subject.as_str())
            || self.incarnation.is_empty()
            || self.incarnation.len() > 256
            || expiry <= issued
            || expiry - issued > GRANT_LIFETIME.as_secs()
            || matches!(&self.owner, ResourceOwner::Person { subject } if subject != &self.subject)
        {
            return Err(BridgeError::Corrupt);
        }
        Ok(())
    }

    fn deadline(&self) -> Result<tokio::time::Instant, BridgeError> {
        self.validate()?;
        let expiry = mongodb::bson::DateTime::parse_rfc3339_str(&self.expires_at)
            .map_err(|_| BridgeError::Corrupt)?
            .to_system_time();
        let remaining = expiry.duration_since(SystemTime::now()).map_err(|_| {
            super::operations::health::authority_failure(
                super::operations::health::AuthorityFailure::ControlGrantExpired,
            );
            reconnect()
        })?;
        Ok(tokio::time::Instant::now() + remaining)
    }
}

/// Never serialized or Debug: the human credential is available only through
/// this exact owner-bound control-plane lease, never a Sandbox runtime DTO.
pub(crate) struct BridgeControlPlaneLease {
    pub(super) grant: BridgeControlPlaneGrant,
    pub(super) human: BridgeHumanLease,
    pub(super) custody_revision: u64,
    pub(super) deadline: tokio::time::Instant,
}

impl BridgeControlPlaneLease {
    #[cfg(test)]
    pub(crate) fn fixture(
        grant: BridgeControlPlaneGrant,
        human: BridgeHumanLease,
        custody_revision: u64,
    ) -> Self {
        grant.validate().unwrap();
        assert_eq!(&grant.subject, human.subject());
        assert_eq!(grant.incarnation, human.incarnation());
        assert!(custody_revision > 0);
        let deadline = grant.deadline().unwrap().min(human.deadline());
        Self {
            grant,
            human,
            custody_revision,
            deadline,
        }
    }

    pub(crate) fn grant(&self) -> &BridgeControlPlaneGrant {
        &self.grant
    }
    pub(crate) fn custody_revision(&self) -> u64 {
        self.custody_revision
    }
    pub(crate) fn owner(&self) -> &ResourceOwner {
        &self.grant.owner
    }
    pub(crate) fn actor_subject(&self) -> &SubjectId {
        self.human.subject()
    }
    pub(crate) fn credential(&self) -> &str {
        self.human.credential()
    }
    pub(crate) fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }
    pub(crate) fn bridge_id(&self) -> &BridgeId {
        &self.grant.bridge_id
    }
    pub(crate) fn connector(&self) -> &BridgeConnector {
        &self.grant.connector
    }
    pub(crate) fn actor_context(&self) -> &layer2_core::resource_access::CallerContext {
        self.human.actor_context()
    }
}

impl ManagedBridgeCredentials {
    pub(crate) async fn resolve_control_plane(
        &self,
        owner: &ResourceOwner,
        config: &BridgeConfig,
    ) -> Result<BridgeControlPlaneLease, BridgeError> {
        let material = self
            .store
            .get(&config.connector.custody)
            .await?
            .ok_or_else(reconnect)?;
        material.metadata.validate()?;
        let grant = material.control_plane.ok_or_else(reconnect)?;
        let deadline = grant.deadline()?;
        if material.metadata.owner != *owner
            || material.metadata.connector != config.connector
            || material.metadata.revoked_at.is_some()
            || grant.owner != *owner
            || grant.connector != config.connector
            || grant.bridge_id != material.metadata.bridge_id
        {
            return Err(reconnect());
        }
        let bridge = self
            .bridges
            .bridge(&grant.bridge_id)
            .await?
            .ok_or_else(reconnect)?;
        if bridge.owner != *owner
            || !bridge
                .generations
                .iter()
                .any(|generation| generation.config == *config)
        {
            return Err(reconnect());
        }
        let human = self
            .auth
            .resolve_bridge_human(&grant.subject, &grant.incarnation, owner)
            .await?;
        let deadline = deadline.min(human.deadline());
        self.provider
            .verify_human(human.subject(), human.credential())
            .await?;
        let lease = BridgeControlPlaneLease {
            grant,
            human,
            custody_revision: material.metadata.revision,
            deadline,
        };
        self.revalidate_control_plane(&lease).await?;
        Ok(lease)
    }

    pub(crate) async fn revalidate_control_plane(
        &self,
        lease: &BridgeControlPlaneLease,
    ) -> Result<(), BridgeError> {
        let current = self
            .store
            .get(&lease.grant.connector.custody)
            .await?
            .ok_or_else(reconnect)?;
        current.metadata.validate()?;
        if current.control_plane.as_ref() != Some(&lease.grant)
            || current.metadata.revision != lease.custody_revision
            || current.metadata.owner != lease.grant.owner
            || current.metadata.connector != lease.grant.connector
            || current.metadata.revoked_at.is_some()
            || tokio::time::Instant::now() >= lease.deadline
        {
            return Err(reconnect());
        }
        self.auth.revalidate_bridge_human(&lease.human).await
    }
}

pub(super) fn reconnect() -> BridgeError {
    BridgeError::ConnectionRequired(BridgeSetupBlock::RenewableAuthorityUnavailable)
}
