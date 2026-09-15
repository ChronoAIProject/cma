//! Current control-plane grant fence for Channel resource transactions. Grant
//! renewal may preserve selected-key revision, so both must be compared.
use super::MongoBridgeCredentialStore;
use crate::bridge::BridgeControlPlaneLease;
use layer2_core::bridge::{BridgeError, BridgeRecord, BridgeSetupBlock};
use mongodb::{bson::doc, ClientSession};

impl MongoBridgeCredentialStore {
    pub(crate) async fn require_channel_invoke_binding(
        &self,
        bridge: &BridgeRecord,
        connector: &layer2_core::bridge::BridgeConnector,
        authority: &dyn layer2_core::bridge::invoke::ChannelInvokeAuthority,
        session: &mut ClientSession,
    ) -> Result<(), ControlPlaneFenceError> {
        let unavailable =
            || BridgeError::ConnectionRequired(BridgeSetupBlock::RenewableAuthorityUnavailable);
        authority.require_current().map_err(|_| unavailable())?;
        let document = self
            .credentials
            .find_one(doc! {"_id":connector.custody.as_str()})
            .session(&mut *session)
            .await?
            .ok_or_else(unavailable)?;
        let current = self.decode(document)?;
        let grant = current.control_plane.as_ref().ok_or_else(unavailable)?;
        let expected = authority
            .control_plane_digest()
            .map_err(|_| unavailable())?;
        let actual =
            crate::bridge::control_plane::invoke::grant_digest(grant).map_err(|_| unavailable())?;
        if actual != expected
            || current.metadata.revision != authority.custody_revision()
            || current.metadata.bridge_id != bridge.id
            || current.metadata.owner != bridge.owner
            || current.metadata.connector != *connector
            || current.metadata.revoked_at.is_some()
            || grant.bridge_id != bridge.id
            || grant.owner != bridge.owner
            || grant.connector != *connector
        {
            return Err(unavailable().into());
        }
        let changed = self.credentials.update_one(
            doc! {"_id":connector.custody.as_str(),"metadata.revision":authority.custody_revision() as i64},
            doc! {"$inc":{"control_plane_fence":1_i64}},
        ).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(BridgeError::Conflict.into());
        }
        authority.require_current().map_err(|_| unavailable())?;
        Ok(())
    }

    pub(crate) async fn require_control_plane_binding(
        &self,
        bridge: &BridgeRecord,
        lease: &BridgeControlPlaneLease,
        session: &mut ClientSession,
    ) -> Result<(), ControlPlaneFenceError> {
        let unavailable =
            || BridgeError::ConnectionRequired(BridgeSetupBlock::RenewableAuthorityUnavailable);
        if tokio::time::Instant::now() >= lease.deadline()
            || lease.bridge_id() != &bridge.id
            || lease.owner() != &bridge.owner
            || !bridge
                .generations
                .iter()
                .any(|generation| generation.config.connector == *lease.connector())
        {
            return Err(unavailable().into());
        }
        let document = self
            .credentials
            .find_one(doc! {"_id":lease.connector().custody.as_str()})
            .session(&mut *session)
            .await?
            .ok_or_else(unavailable)?;
        let current = self.decode(document)?;
        if current.control_plane.as_ref() != Some(lease.grant())
            || current.metadata.revision != lease.custody_revision()
            || current.metadata.bridge_id != bridge.id
            || current.metadata.owner != bridge.owner
            || current.metadata.connector != *lease.connector()
            || current.metadata.revoked_at.is_some()
        {
            return Err(unavailable().into());
        }
        let changed = self.credentials.update_one(doc! {"_id":lease.connector().custody.as_str(),"metadata.revision":lease.custody_revision() as i64}, doc! {"$inc":{"control_plane_fence":1_i64}}).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(BridgeError::Conflict.into());
        }
        let changed = self
            .definitions
            .update_one(
                doc! {"_id":bridge.id.as_str(),"revision":bridge.revision as i64},
                doc! {"$inc":{"control_plane_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(BridgeError::Conflict.into());
        }
        if tokio::time::Instant::now() >= lease.deadline() {
            return Err(unavailable().into());
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ControlPlaneFenceError {
    #[error(transparent)]
    Core(#[from] BridgeError),
    #[error(transparent)]
    Driver(#[from] mongodb::error::Error),
}
